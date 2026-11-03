//! MIDI input from every available port.

use std::sync::mpsc::{Receiver, channel};

use midir::{MidiInput, MidiInputConnection};

use crate::{Error, Result};

/// Raw MIDI bytes and the port they came from.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiEvent {
    pub port: String,
    pub bytes: Vec<u8>,
}

/// Open connections to MIDI input ports. Dropping it closes them.
pub struct MidiInputs {
    connections: Vec<MidiInputConnection<()>>,
    ports: Vec<String>,
    events: Receiver<MidiEvent>,
}

impl MidiInputs {
    /// Connects to every MIDI input port present now. Ports that fail to open
    /// are skipped and listed in the returned errors.
    pub fn open_all() -> Result<(Self, Vec<String>)> {
        let probe = MidiInput::new("Verso").map_err(|e| Error::Midi(e.to_string()))?;
        let (sender, events) = channel();
        let mut connections = Vec::new();
        let mut ports = Vec::new();
        let mut errors = Vec::new();
        for port in probe.ports() {
            let name = probe
                .port_name(&port)
                .unwrap_or_else(|_| "MIDI port".into());
            // Each connection consumes its own MidiInput.
            let input = match MidiInput::new("Verso") {
                Ok(input) => input,
                Err(e) => {
                    errors.push(format!("{name}: {e}"));
                    continue;
                }
            };
            // Receive MIDI Time Code (quarter frames and SysEx) too.
            let mut input = input;
            input.ignore(midir::Ignore::ActiveSense);
            let sender = sender.clone();
            let port_name = name.clone();
            match input.connect(
                &port,
                "verso-in",
                move |_, bytes, _| {
                    let _ = sender.send(MidiEvent {
                        port: port_name.clone(),
                        bytes: bytes.to_vec(),
                    });
                },
                (),
            ) {
                Ok(connection) => {
                    connections.push(connection);
                    ports.push(name);
                }
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        Ok((
            Self {
                connections,
                ports,
                events,
            },
            errors,
        ))
    }

    /// Names of the connected ports.
    pub fn ports(&self) -> &[String] {
        &self.ports
    }

    /// Messages received since the last call.
    pub fn poll(&self) -> Vec<MidiEvent> {
        self.events.try_iter().collect()
    }

    pub fn len(&self) -> usize {
        self.connections.len()
    }

    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }
}
