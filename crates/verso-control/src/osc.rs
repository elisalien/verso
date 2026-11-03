//! OSC input over UDP.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TrySendError, sync_channel};
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

/// Messages waiting for [`OscServer::poll`]; more are dropped.
const QUEUE: usize = 4096;
/// Bundles nested deeper than this are dropped (the decoder recurses).
const MAX_BUNDLES: usize = 64;

/// Who may send OSC.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OscAccess {
    /// Only programs on this computer (127.0.0.1).
    #[default]
    Local,
    /// Any computer on the network, optionally only from these addresses
    /// (empty allows all).
    Network { allowed: Vec<IpAddr> },
}

/// Listens for OSC on a UDP port until dropped.
pub struct OscServer {
    port: u16,
    events: Receiver<OscEvent>,
    stop: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}

/// Number of bundles in a packet, counted without decoding it.
fn bundle_count(bytes: &[u8]) -> usize {
    bytes.windows(8).filter(|w| *w == b"#bundle\0").count()
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
    /// Listens on `127.0.0.1:port`. Port 0 picks a free port; see [`Self::port`].
    pub fn bind(port: u16) -> Result<Self> {
        Self::bind_with(port, OscAccess::Local)
    }

    /// Listens on `port` for the senders `access` allows.
    pub fn bind_with(port: u16, access: OscAccess) -> Result<Self> {
        let (host, allowed) = match access {
            OscAccess::Local => (IpAddr::V4(Ipv4Addr::LOCALHOST), Vec::new()),
            OscAccess::Network { allowed } => (IpAddr::V4(Ipv4Addr::UNSPECIFIED), allowed),
        };
        let socket =
            UdpSocket::bind((host, port)).map_err(|e| Error::Osc(format!("port {port}: {e}")))?;
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
        let (sender, events) = sync_channel(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_count = dropped.clone();
        thread::Builder::new()
            .name("verso-osc".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let mut buffer = vec![0u8; 65536];
                while !stopped.load(Ordering::Relaxed) {
                    let len = match socket.recv_from(&mut buffer) {
                        Ok((_, from)) if !allowed.is_empty() && !allowed.contains(&from.ip()) => {
                            dropped_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        Ok((len, _)) => len,
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
                    if bundle_count(&buffer[..len]) > MAX_BUNDLES {
                        dropped_count.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let Ok((_, packet)) = rosc::decoder::decode_udp(&buffer[..len]) else {
                        continue;
                    };
                    let mut messages = Vec::new();
                    flatten(packet, &mut messages);
                    for message in messages {
                        match sender.try_send(message) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => {
                                dropped_count.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(TrySendError::Disconnected(_)) => return,
                        }
                    }
                }
            })
            .map_err(|e| Error::Osc(e.to_string()))?;
        Ok(Self {
            port,
            events,
            stop,
            dropped,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Packets refused (sender not allowed, too deep) or messages dropped
    /// because the queue was full, since the server started.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
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
    fn deeply_nested_bundles_are_dropped() {
        let server = OscServer::bind(0).unwrap();
        let mut packet = OscPacket::Message(OscMessage {
            addr: "/clear".into(),
            args: vec![],
        });
        for _ in 0..(MAX_BUNDLES + 10) {
            packet = OscPacket::Bundle(OscBundle {
                timetag: OscTime::from((0, 1)),
                content: vec![packet],
            });
        }
        send(server.port(), &packet);
        std::thread::sleep(Duration::from_millis(300));
        assert!(server.poll().is_empty());
        assert_eq!(server.dropped(), 1);
    }

    #[test]
    fn senders_outside_the_allow_list_are_ignored() {
        let server = OscServer::bind_with(
            0,
            OscAccess::Network {
                allowed: vec!["10.9.8.7".parse().unwrap()],
            },
        )
        .unwrap();
        send(
            server.port(),
            &OscPacket::Message(OscMessage {
                addr: "/clear".into(),
                args: vec![],
            }),
        );
        std::thread::sleep(Duration::from_millis(300));
        assert!(server.poll().is_empty());
        assert_eq!(server.dropped(), 1);
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
