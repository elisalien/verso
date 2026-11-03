//! Pointer and keyboard gestures for the `ui_input` tool, independent of the
//! UI toolkit. Coordinates are UI points, as `ui_widgets` reports them.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Primary,
    Secondary,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Gesture {
    /// Leaves the pointer there, so the UI sees a hover.
    Move {
        at: [f32; 2],
    },
    Click {
        at: [f32; 2],
        button: Button,
        count: u8,
        modifiers: Modifiers,
    },
    /// Press at `from`, move in steps, release at `to`: drag and drop.
    Drag {
        from: [f32; 2],
        to: [f32; 2],
        modifiers: Modifiers,
    },
    /// Wheel at `at`; positive `delta[1]` scrolls up, as a wheel turned away.
    Scroll {
        at: [f32; 2],
        delta: [f32; 2],
    },
    /// A key by its egui name (`Escape`, `Enter`, `A`, `F2`, `1`…).
    Key {
        name: String,
        modifiers: Modifiers,
    },
    Text(String),
    /// The pointer leaves the window.
    Leave,
}

fn point(args: &Value, x: &str, y: &str) -> Result<[f32; 2], String> {
    match (args[x].as_f64(), args[y].as_f64()) {
        (Some(x), Some(y)) => Ok([x as f32, y as f32]),
        _ => Err(format!("{x} and {y} are required, in UI points")),
    }
}

fn modifiers(args: &Value) -> Modifiers {
    let list: Vec<String> = args["modifiers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m.as_str().map(str::to_lowercase))
        .collect();
    let has = |names: &[&str]| list.iter().any(|m| names.contains(&m.as_str()));
    Modifiers {
        ctrl: has(&["ctrl", "control", "command", "cmd"]),
        shift: has(&["shift"]),
        alt: has(&["alt", "option"]),
    }
}

/// Reads the arguments of `ui_input`.
pub fn parse(args: &Value) -> Result<Gesture, String> {
    let action = args["action"].as_str().ok_or("action is required")?;
    let click = |button, count| -> Result<Gesture, String> {
        Ok(Gesture::Click {
            at: point(args, "x", "y")?,
            button,
            count,
            modifiers: modifiers(args),
        })
    };
    match action {
        "move" => Ok(Gesture::Move {
            at: point(args, "x", "y")?,
        }),
        "click" => click(Button::Primary, 1),
        "double_click" => click(Button::Primary, 2),
        "right_click" => click(Button::Secondary, 1),
        "middle_click" => click(Button::Middle, 1),
        "drag" => Ok(Gesture::Drag {
            from: point(args, "x", "y")?,
            to: point(args, "to_x", "to_y")?,
            modifiers: modifiers(args),
        }),
        "scroll" => Ok(Gesture::Scroll {
            at: point(args, "x", "y")?,
            delta: [
                args["dx"].as_f64().unwrap_or(0.0) as f32,
                args["dy"].as_f64().unwrap_or(0.0) as f32,
            ],
        }),
        "key" => Ok(Gesture::Key {
            name: args["key"]
                .as_str()
                .ok_or("key is required, e.g. Escape, Enter, F2, A")?
                .to_owned(),
            modifiers: modifiers(args),
        }),
        "text" => Ok(Gesture::Text(
            args["text"].as_str().ok_or("text is required")?.to_owned(),
        )),
        "leave" => Ok(Gesture::Leave),
        other => Err(format!(
            "unknown action {other}: move, click, double_click, right_click, middle_click, drag, scroll, key, text or leave"
        )),
    }
}

/// Port asked for with `--mcp [port]`, `--mcp=port`, or else the
/// `VERSO_MCP_PORT` value. `None` keeps the server off.
pub fn requested_port(args: &[String], env: Option<&str>) -> Option<u16> {
    for (i, arg) in args.iter().enumerate() {
        if let Some(port) = arg.strip_prefix("--mcp=") {
            return port.parse().ok();
        }
        if arg == "--mcp" {
            return Some(
                args.get(i + 1)
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(crate::DEFAULT_PORT),
            );
        }
    }
    env?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gestures_parse() {
        assert_eq!(
            parse(&json!({"action": "right_click", "x": 10, "y": 20.5})).unwrap(),
            Gesture::Click {
                at: [10.0, 20.5],
                button: Button::Secondary,
                count: 1,
                modifiers: Modifiers::default()
            }
        );
        assert_eq!(
            parse(
                &json!({"action": "drag", "x": 1, "y": 2, "to_x": 3, "to_y": 4,
                          "modifiers": ["Ctrl"]})
            )
            .unwrap(),
            Gesture::Drag {
                from: [1.0, 2.0],
                to: [3.0, 4.0],
                modifiers: Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                }
            }
        );
        assert_eq!(
            parse(&json!({"action": "key", "key": "Z", "modifiers": ["ctrl", "shift"]})).unwrap(),
            Gesture::Key {
                name: "Z".into(),
                modifiers: Modifiers {
                    ctrl: true,
                    shift: true,
                    alt: false
                }
            }
        );
        assert!(parse(&json!({"action": "click"})).is_err());
        assert!(parse(&json!({"action": "wave"})).is_err());
    }

    #[test]
    fn port_from_arguments_or_environment() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(requested_port(&args(&[]), None), None);
        assert_eq!(
            requested_port(&args(&["--mcp"]), None),
            Some(crate::DEFAULT_PORT)
        );
        assert_eq!(requested_port(&args(&["--mcp", "7411"]), None), Some(7411));
        assert_eq!(requested_port(&args(&["--mcp=7412"]), None), Some(7412));
        assert_eq!(requested_port(&args(&[]), Some("7413")), Some(7413));
        assert_eq!(
            requested_port(&args(&["--mcp", "file.verso"]), None),
            Some(crate::DEFAULT_PORT)
        );
    }
}
