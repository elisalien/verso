//! Model Context Protocol server for Verso.
//!
//! An agent connects over the streamable HTTP transport on `127.0.0.1` and
//! calls tools. The HTTP threads never touch the composition: each tool call
//! becomes a [`ToolCall`] that the app picks up with [`McpServer::poll`] at the
//! start of a frame and answers with [`ToolCall::reply`]. See `docs/MCP.md`.

pub mod gesture;
mod http;
pub mod protocol;
pub mod summary;
pub mod tools;

use std::sync::mpsc::Sender;

pub use gesture::requested_port;
pub use http::McpServer;
use serde_json::Value;

/// Default port of `verso --mcp`.
pub const DEFAULT_PORT: u16 = 7400;

/// One item of a tool result.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(String),
    /// PNG file bytes.
    Png(Vec<u8>),
}

/// What a tool call returns.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub content: Vec<Content>,
    /// The tool ran but failed; the text says why.
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(text.into())],
            is_error: false,
        }
    }

    /// Pretty-printed JSON as text.
    pub fn json(value: &Value) -> Self {
        Self::text(serde_json::to_string_pretty(value).unwrap_or_default())
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(text.into())],
            is_error: true,
        }
    }

    pub fn png(bytes: Vec<u8>, caption: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Png(bytes), Content::Text(caption.into())],
            is_error: false,
        }
    }

    /// The result object of a `tools/call` response.
    pub fn to_json(&self) -> Value {
        let content: Vec<Value> = self
            .content
            .iter()
            .map(|c| match c {
                Content::Text(text) => serde_json::json!({"type": "text", "text": text}),
                Content::Png(bytes) => serde_json::json!({
                    "type": "image",
                    "data": base64(bytes),
                    "mimeType": "image/png",
                }),
            })
            .collect();
        serde_json::json!({"content": content, "isError": self.is_error})
    }
}

/// A tool call waiting for the app.
pub struct ToolCall {
    pub name: String,
    /// The `arguments` object, `{}` when the client sent none.
    pub args: Value,
    reply: Sender<ToolOutput>,
}

impl ToolCall {
    pub fn new(name: String, args: Value, reply: Sender<ToolOutput>) -> Self {
        Self { name, args, reply }
    }

    /// Answers the call. The client may have given up waiting; that is fine.
    pub fn reply(self, output: ToolOutput) {
        let _ = self.reply.send(output);
    }
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (input, expected) in cases {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }

    #[test]
    fn images_become_image_content() {
        let json = ToolOutput::png(vec![1, 2, 3], "out").to_json();
        assert_eq!(json["content"][0]["type"], "image");
        assert_eq!(json["content"][0]["mimeType"], "image/png");
        assert_eq!(json["content"][0]["data"], "AQID");
        assert_eq!(json["content"][1]["text"], "out");
        assert_eq!(json["isError"], false);
    }
}
