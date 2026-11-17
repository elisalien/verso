//! JSON-RPC messages of the MCP protocol, independent of the transport.

use serde_json::{Value, json};

use crate::{ToolOutput, tools};

/// Protocol revisions this server speaks, newest first.
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Verso is a VJ application: a grid of clips (layers \
are rows, counted from the bottom; columns are scenes). Indices in tool \
arguments start at 1, as in Verso's OSC addresses; apply_commands takes raw \
Command values, which count from 0. Start with status and get_composition. \
Use capture to see the output or a layer, column or clip rendered on the GPU. \
Destructive tools need \"confirm\": true; ask the user first.";

fn error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Handles one JSON-RPC message (or a batch). Returns the response, or
/// `None` for notifications and responses, which get none. `call` runs a
/// tool that passed the checks.
pub fn handle(message: &Value, call: &dyn Fn(String, Value) -> ToolOutput) -> Option<Value> {
    if let Some(batch) = message.as_array() {
        let replies: Vec<Value> = batch.iter().filter_map(|m| handle(m, call)).collect();
        return (!replies.is_empty()).then_some(Value::Array(replies));
    }
    let Some(object) = message.as_object() else {
        return Some(error(&Value::Null, -32600, "expected a JSON-RPC object"));
    };
    let id = object.get("id")?.clone();
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // A response from the client: nothing to answer.
        return None;
    };
    let params = object.get("params").cloned().unwrap_or(json!({}));
    Some(match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = SUPPORTED_VERSIONS
                .iter()
                .find(|v| **v == asked)
                .unwrap_or(&SUPPORTED_VERSIONS[0]);
            result(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "verso", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => result(&id, json!({})),
        "tools/list" => result(&id, json!({"tools": tools::list()})),
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return Some(error(&id, -32602, "missing tool name"));
            };
            if tools::find(name).is_none() {
                return Some(error(&id, -32602, format!("unknown tool {name}")));
            }
            let args = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(args @ Value::Object(_)) => args.clone(),
                Some(_) => return Some(error(&id, -32602, "arguments must be an object")),
            };
            let output = match tools::check(name, &args) {
                Ok(()) => call(name.to_owned(), args),
                Err(refusal) => ToolOutput::error(refusal),
            };
            result(&id, output.to_json())
        }
        other => error(&id, -32601, format!("method {other} is not supported")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo(name: String, args: Value) -> ToolOutput {
        ToolOutput::text(format!("{name} {args}"))
    }

    #[test]
    fn initialize_negotiates_the_version() {
        let reply = handle(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": {"protocolVersion": "2025-03-26"}}),
            &echo,
        )
        .unwrap();
        assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(reply["result"]["serverInfo"]["name"], "verso");
        let reply = handle(
            &json!({"jsonrpc": "2.0", "id": 2, "method": "initialize",
                    "params": {"protocolVersion": "1999-01-01"}}),
            &echo,
        )
        .unwrap();
        assert_eq!(reply["result"]["protocolVersion"], SUPPORTED_VERSIONS[0]);
    }

    #[test]
    fn notifications_get_no_reply() {
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert_eq!(handle(&note, &echo), None);
    }

    #[test]
    fn tools_are_listed_with_schemas() {
        let reply = handle(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            &echo,
        )
        .unwrap();
        let tools = reply["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "capture"));
        assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
    }

    #[test]
    fn calls_reach_the_handler() {
        let reply = handle(
            &json!({"jsonrpc": "2.0", "id": "a", "method": "tools/call",
                    "params": {"name": "osc", "arguments": {"address": "/clear"}}}),
            &echo,
        )
        .unwrap();
        assert_eq!(reply["id"], "a");
        assert_eq!(reply["result"]["isError"], false);
        assert_eq!(
            reply["result"]["content"][0]["text"],
            r#"osc {"address":"/clear"}"#
        );
    }

    #[test]
    fn destructive_calls_need_confirm() {
        let call = |args: Value| {
            handle(
                &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": {"name": "save", "arguments": args}}),
                &echo,
            )
            .unwrap()
        };
        assert_eq!(call(json!({}))["result"]["isError"], true);
        assert_eq!(call(json!({"confirm": true}))["result"]["isError"], false);
    }

    #[test]
    fn unknown_methods_and_tools_are_errors() {
        let reply = handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "nope"}), &echo).unwrap();
        assert_eq!(reply["error"]["code"], -32601);
        let reply = handle(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "nope"}}),
            &echo,
        )
        .unwrap();
        assert_eq!(reply["error"]["code"], -32602);
    }
}
