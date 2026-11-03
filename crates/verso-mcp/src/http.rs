//! Streamable HTTP transport on `127.0.0.1`: one JSON response per POST, no
//! event stream, no session. A thread per connection; connections stay open
//! between requests.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::{ToolCall, ToolOutput, protocol};

/// Path of the MCP endpoint.
pub const ENDPOINT: &str = "/mcp";
/// Longest wait for the app to answer a tool call.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Idle time after which a kept-alive connection is closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_HEADERS: usize = 64 * 1024;
const MAX_BODY: usize = 16 * 1024 * 1024;

/// Listens on `127.0.0.1:port` until dropped.
pub struct McpServer {
    port: u16,
    calls: Receiver<ToolCall>,
    stop: Arc<AtomicBool>,
}

struct Shared {
    port: u16,
    calls: Sender<ToolCall>,
    /// Wakes the app so it polls even while idle.
    wake: Box<dyn Fn() + Send + Sync>,
    stop: Arc<AtomicBool>,
}

impl McpServer {
    /// Binds the loopback interface only. Port 0 picks a free port; see
    /// [`Self::port`]. `wake` is called after each tool call is queued.
    pub fn bind(port: u16, wake: impl Fn() + Send + Sync + 'static) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, calls) = channel();
        let shared = Arc::new(Shared {
            port,
            calls: sender,
            wake: Box::new(wake),
            stop: stop.clone(),
        });
        thread::Builder::new()
            .name("verso-mcp".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if shared.stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(stream) = stream else { continue };
                    let shared = shared.clone();
                    let _ = thread::Builder::new()
                        .name("verso-mcp-conn".into())
                        .spawn(move || serve(stream, &shared));
                }
            })?;
        Ok(Self { port, calls, stop })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Tool calls received since the last poll.
    pub fn poll(&self) -> Vec<ToolCall> {
        self.calls.try_iter().collect()
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblocks `accept` so the thread sees `stop` and frees the port.
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port));
    }
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Reads one request; `Ok(None)` when the client closed the connection.
fn read_request(reader: &mut impl BufRead) -> io::Result<Option<Request>> {
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(bad("bad request line"));
    };
    let path = target.split('?').next().unwrap_or_default().to_owned();
    let method = method.to_owned();
    let mut headers = Vec::new();
    let mut total = line.len();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(bad("connection closed in headers"));
        }
        total += line.len();
        if total > MAX_HEADERS {
            return Err(bad("headers too large"));
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    let mut request = Request {
        method,
        path,
        headers,
        body: Vec::new(),
    };
    let length: usize = request
        .header("content-length")
        .map(|v| v.parse().map_err(|_| bad("bad content-length")))
        .transpose()?
        .unwrap_or(0);
    if length > MAX_BODY {
        return Err(bad("body too large"));
    }
    request.body.resize(length, 0);
    reader.read_exact(&mut request.body)?;
    Ok(Some(request))
}

/// Whether `value` names this machine, with or without a port.
fn is_local_host(value: &str, port: Option<u16>) -> bool {
    let (host, rest) = match value.strip_prefix('[') {
        Some(v6) => match v6.split_once(']') {
            Some((host, rest)) => (host, rest),
            None => return false,
        },
        None => match value.rsplit_once(':') {
            Some((host, _)) => (host, &value[host.len()..]),
            None => (value, ""),
        },
    };
    let host_ok = matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    );
    let port_ok = match (rest.strip_prefix(':'), port) {
        (None, _) => rest.is_empty(),
        (Some(p), Some(port)) => p.parse::<u16>() == Ok(port),
        (Some(p), None) => p.parse::<u16>().is_ok(),
    };
    host_ok && port_ok
}

/// DNS rebinding guard: the browser of a remote page could reach the
/// loopback port under another host name; it cannot fake these headers.
fn allowed(request: &Request, port: u16) -> bool {
    let host_ok = request
        .header("host")
        .is_some_and(|h| is_local_host(h, Some(port)));
    let origin_ok = match request.header("origin") {
        None => true,
        Some(origin) => origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"))
            .is_some_and(|rest| is_local_host(rest.trim_end_matches('/'), None)),
    };
    host_ok && origin_ok
}

struct Response {
    status: &'static str,
    content_type: Option<&'static str>,
    extra: Option<&'static str>,
    body: Vec<u8>,
}

impl Response {
    fn empty(status: &'static str) -> Self {
        Self {
            status,
            content_type: None,
            extra: None,
            body: Vec::new(),
        }
    }

    fn json(status: &'static str, value: &Value) -> Self {
        Self {
            status,
            content_type: Some("application/json"),
            extra: None,
            body: serde_json::to_vec(value).unwrap_or_default(),
        }
    }

    fn text(status: &'static str, text: &str) -> Self {
        Self {
            status,
            content_type: Some("text/plain; charset=utf-8"),
            extra: None,
            body: text.as_bytes().to_vec(),
        }
    }
}

fn respond(request: &Request, shared: &Shared) -> Response {
    if request.path != ENDPOINT {
        return Response::text("404 Not Found", "The MCP endpoint is /mcp.");
    }
    if !allowed(request, shared.port) {
        return Response::text("403 Forbidden", "Only local clients may connect.");
    }
    match request.method.as_str() {
        "POST" => {}
        "GET" | "DELETE" => {
            return Response {
                extra: Some("Allow: POST\r\n"),
                ..Response::empty("405 Method Not Allowed")
            };
        }
        _ => return Response::empty("405 Method Not Allowed"),
    }
    let message: Value = match serde_json::from_slice(&request.body) {
        Ok(message) => message,
        Err(e) => {
            return Response::json(
                "400 Bad Request",
                &serde_json::json!({"jsonrpc": "2.0", "id": null,
                    "error": {"code": -32700, "message": format!("parse error: {e}")}}),
            );
        }
    };
    let call = |name: String, args: Value| {
        let (reply, answer) = channel();
        if shared.calls.send(ToolCall::new(name, args, reply)).is_err() {
            return ToolOutput::error("Verso is shutting down.");
        }
        (shared.wake)();
        answer.recv_timeout(CALL_TIMEOUT).unwrap_or_else(|_| {
            ToolOutput::error("Verso did not answer in time (is the window frozen?).")
        })
    };
    match protocol::handle(&message, &call) {
        Some(reply) => Response::json("200 OK", &reply),
        None => Response::empty("202 Accepted"),
    }
}

fn write_response(stream: &mut impl Write, response: &Response, close: bool) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: {}\r\n",
        response.status,
        response.body.len(),
        if close { "close" } else { "keep-alive" }
    );
    if let Some(content_type) = response.content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    if let Some(extra) = response.extra {
        head.push_str(extra);
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

fn serve(stream: TcpStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(IDLE_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    while !shared.stop.load(Ordering::Relaxed) {
        let request = match read_request(&mut reader) {
            Ok(Some(request)) => request,
            Ok(None) => return,
            Err(e) => {
                if e.kind() == io::ErrorKind::InvalidData {
                    let _ = write_response(
                        &mut writer,
                        &Response::text("400 Bad Request", &e.to_string()),
                        true,
                    );
                }
                return;
            }
        };
        let close = request
            .header("connection")
            .is_some_and(|c| c.eq_ignore_ascii_case("close"));
        let response = respond(&request, shared);
        if write_response(&mut writer, &response, close).is_err() || close {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_hosts() {
        assert!(is_local_host("127.0.0.1:7400", Some(7400)));
        assert!(is_local_host("localhost:7400", Some(7400)));
        assert!(is_local_host("[::1]:7400", Some(7400)));
        assert!(!is_local_host("127.0.0.1:7401", Some(7400)));
        assert!(!is_local_host("evil.example:7400", Some(7400)));
        assert!(!is_local_host("127.0.0.1.evil.example:7400", Some(7400)));
        assert!(is_local_host("localhost", None));
        assert!(is_local_host("localhost:3000", None));
        assert!(!is_local_host("localhost.evil:3000", None));
    }
}
