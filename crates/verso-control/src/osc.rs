//! OSC input over UDP.

use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;

use rosc::{OscPacket, OscType};

use crate::{Error, Result};

/// One OSC message, reduced to its address and first numeric argument.
#[derive(Debug, Clone, PartialEq)]
pub struct OscEvent {
    pub address: String,
    /// `None` for messages without a numeric argument (pure triggers).
    pub value: Option<f32>,
}

/// Listens for OSC on a UDP port until dropped.
pub struct OscServer {
    port: u16,
    events: Receiver<OscEvent>,
    stop: Arc<AtomicBool>,
}

fn number(arg: &OscType) -> Option<f32> {
    match arg {
        OscType::Float(v) => Some(*v),
        OscType::Double(v) => Some(*v as f32),
        OscType::Int(v) => Some(*v as f32),
        OscType::Long(v) => Some(*v as f32),
        OscType::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn flatten(packet: OscPacket, out: &mut Vec<OscEvent>) {
    match packet {
        OscPacket::Message(msg) => out.push(OscEvent {
            value: msg.args.first().and_then(number),
            address: msg.addr,
        }),
        OscPacket::Bundle(bundle) => {
            for packet in bundle.content {
                flatten(packet, out);
            }
        }
    }
}

impl OscServer {
    /// Binds `0.0.0.0:port`. Port 0 picks a free port; see [`Self::port`].
    pub fn bind(port: u16) -> Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", port))
            .map_err(|e| Error::Osc(format!("port {port}: {e}")))?;
        let port = socket
            .local_addr()
            .map_err(|e| Error::Osc(e.to_string()))?
            .port();
        // A timeout lets the thread notice `stop` while no packet arrives.
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|e| Error::Osc(e.to_string()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (sender, events) = channel();
        thread::Builder::new()
            .name("verso-osc".into())
            .spawn(move || {
                let mut buffer = [0u8; 65536];
                while !stopped.load(Ordering::Relaxed) {
                    let len = match socket.recv(&mut buffer) {
                        Ok(len) => len,
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            continue;
                        }
                        Err(_) => return,
                    };
                    let Ok((_, packet)) = rosc::decoder::decode_udp(&buffer[..len]) else {
                        continue;
                    };
                    let mut messages = Vec::new();
                    flatten(packet, &mut messages);
                    if messages.into_iter().any(|m| sender.send(m).is_err()) {
                        return;
                    }
                }
            })
            .map_err(|e| Error::Osc(e.to_string()))?;
        Ok(Self { port, events, stop })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Messages received since the last call.
    pub fn poll(&self) -> Vec<OscEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for OscServer {
    fn drop(&mut self) {
        // The thread exits within one read timeout and frees the port.
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{OscBundle, OscMessage, OscTime};
    use std::time::{Duration, Instant};

    fn send(port: u16, packet: &OscPacket) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let bytes = rosc::encoder::encode(packet).unwrap();
        socket.send_to(&bytes, ("127.0.0.1", port)).unwrap();
    }

    fn wait(server: &OscServer, count: usize) -> Vec<OscEvent> {
        let start = Instant::now();
        let mut events = Vec::new();
        while events.len() < count && start.elapsed() < Duration::from_secs(5) {
            events.extend(server.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        events
    }

    #[test]
    fn receives_messages_and_bundles() {
        let server = OscServer::bind(0).unwrap();
        assert_ne!(server.port(), 0);
        send(
            server.port(),
            &OscPacket::Message(OscMessage {
                addr: "/layer/1/opacity".into(),
                args: vec![OscType::Float(0.25)],
            }),
        );
        send(
            server.port(),
            &OscPacket::Bundle(OscBundle {
                timetag: OscTime::from((0, 1)),
                content: vec![
                    OscPacket::Message(OscMessage {
                        addr: "/clear".into(),
                        args: vec![],
                    }),
                    OscPacket::Message(OscMessage {
                        addr: "/bpm".into(),
                        args: vec![OscType::Int(128)],
                    }),
                ],
            }),
        );
        let events = wait(&server, 3);
        assert_eq!(
            events,
            [
                OscEvent {
                    address: "/layer/1/opacity".into(),
                    value: Some(0.25)
                },
                OscEvent {
                    address: "/clear".into(),
                    value: None
                },
                OscEvent {
                    address: "/bpm".into(),
                    value: Some(128.0)
                },
            ]
        );
    }

    #[test]
    fn busy_port_is_an_error() {
        let first = OscServer::bind(0).unwrap();
        let second = OscServer::bind(first.port());
        // Some systems allow sharing; when they do not, the error names the port.
        if let Err(e) = second {
            assert!(e.to_string().contains(&first.port().to_string()));
        }
    }
}
