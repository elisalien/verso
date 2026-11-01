//! The server over real sockets, with a thread standing in for the app.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use verso_core::{Command, EffectEdit, EffectTarget, ParamValue};
use verso_mcp::{McpServer, ToolOutput};

/// Sends one request and returns the status code and body.
fn request(port: u16, method: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut head = format!(
        "{method} /mcp HTTP/1.1\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if !headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("host")) {
        head.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body.as_bytes()).unwrap();
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).unwrap();
    let code = status.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    (code, String::from_utf8(body).unwrap())
}

fn post(port: u16, message: Value) -> Value {
    let (code, body) = request(port, "POST", &[], &message.to_string());
    assert_eq!(code, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

/// Answers tool calls like the app would, until `server` is dropped.
fn fake_app(server: McpServer) -> u16 {
    let port = server.port();
    thread::spawn(move || {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            for call in server.poll() {
                let text = format!("{} {}", call.name, call.args);
                call.reply(ToolOutput::text(text));
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    port
}

#[test]
fn a_session_over_http() {
    let port = fake_app(McpServer::bind(0, || {}).unwrap());
    let init = post(
        port,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "test", "version": "0"}}}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    let (code, body) = request(
        port,
        "POST",
        &[],
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    assert_eq!((code, body.as_str()), (202, ""));

    let list = post(
        port,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    );
    assert!(list["result"]["tools"].as_array().unwrap().len() > 10);

    let call = post(
        port,
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
               "params": {"name": "osc", "arguments": {"address": "/column/1/launch"}}}),
    );
    assert_eq!(
        call["result"]["content"][0]["text"],
        r#"osc {"address":"/column/1/launch"}"#
    );

    // Refused before reaching the app.
    let call = post(
        port,
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
               "params": {"name": "apply_commands", "arguments": {"commands": [{"RemoveLayer": 0}]}}}),
    );
    assert_eq!(call["result"]["isError"], true);
}

#[test]
fn foreign_hosts_and_origins_are_refused() {
    let port = fake_app(McpServer::bind(0, || {}).unwrap());
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let (code, _) = request(port, "POST", &[("Host", "evil.example")], ping);
    assert_eq!(code, 403);
    let (code, _) = request(port, "POST", &[("Origin", "http://evil.example")], ping);
    assert_eq!(code, 403);
    let (code, _) = request(port, "POST", &[("Origin", "http://localhost:5173")], ping);
    assert_eq!(code, 200);
    let (code, _) = request(port, "GET", &[], "");
    assert_eq!(code, 405);
    let (code, _) = request(port, "POST", &[], "{not json");
    assert_eq!(code, 400);
}

#[test]
fn listens_on_loopback_only() {
    let server = McpServer::bind(0, || {}).unwrap();
    let port = server.port();
    // Some address of this machine that is not loopback, if it has one.
    let external = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip());
    if let Ok(ip) = external
        && !ip.is_loopback()
        && !ip.is_unspecified()
    {
        assert!(
            TcpStream::connect_timeout(&(ip, port).into(), Duration::from_millis(500)).is_err()
        );
    }
    drop(server);
}

#[test]
fn commands_travel_as_json() {
    let commands = vec![
        Command::ClearAll,
        Command::TriggerClip {
            layer: 0,
            column: 2,
        },
        Command::RemoveLayer(1),
        Command::EditEffects {
            target: EffectTarget::Clip {
                layer: 1,
                column: 0,
            },
            edit: EffectEdit::SetParam {
                index: 0,
                name: "amount".into(),
                value: ParamValue::Float(0.5),
            },
        },
    ];
    let json = serde_json::to_value(&commands).unwrap();
    assert_eq!(json[0], "ClearAll");
    assert_eq!(json[1], json!({"TriggerClip": {"layer": 0, "column": 2}}));
    let back: Vec<Command> = serde_json::from_value(json).unwrap();
    assert_eq!(back, commands);
}
