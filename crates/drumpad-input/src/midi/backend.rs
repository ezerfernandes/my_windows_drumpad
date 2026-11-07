//! MIDI input backends: a fake backend for tests and a midir backend for real
//! hardware.
//!
//! The trait is deliberately small: enumerate ports, open one with a callback,
//! and drop/close it. `drumpad-app` owns the backend choice; tests and the
//! deterministic harness use [`FakeMidiBackend`] so no hardware is needed.
//!
//! The callback receives `(timestamp_us, bytes)` exactly as midir delivers
//! them. Timestamps are the driver's microsecond clock; the capture path uses
//! its own monotonic clock for queue ordering and records driver timestamps
//! separately for latency reports.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A MIDI input port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiPortInfo {
    /// Index in the backend's current enumeration.
    pub index: usize,
    /// Display name, e.g. `"Alesis Nitro Max"`.
    pub name: String,
    /// Opaque backend identifier, when the backend has one.
    pub id: Option<String>,
}

impl MidiPortInfo {
    /// Build a port descriptor.
    #[must_use]
    pub fn new(index: usize, name: impl Into<String>) -> Self {
        Self {
            index,
            name: name.into(),
            id: None,
        }
    }
}

/// Typed MIDI failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MidiError {
    /// The MIDI subsystem could not be initialized.
    #[error("MIDI is unavailable: {0}")]
    Unavailable(String),

    /// The requested port is no longer present.
    #[error("MIDI input port \"{name}\" was not found")]
    PortNotFound {
        /// Port name that was requested.
        name: String,
    },

    /// The port exists but could not be opened.
    #[error("could not open MIDI input port \"{name}\": {reason}")]
    OpenFailed {
        /// Port name.
        name: String,
        /// Backend explanation.
        reason: String,
    },

    /// The backend had no handler for the message.
    #[error("MIDI input port \"{name}\" was disconnected")]
    Disconnected {
        /// Port name.
        name: String,
    },
}

/// Callback invoked for every received message.
pub type MidiHandler = Box<dyn FnMut(u64, &[u8]) + Send>;

/// An open MIDI input connection.
pub trait MidiConnection: Send {
    /// Port descriptor the connection was opened for.
    fn port(&self) -> &MidiPortInfo;

    /// Close the connection. Must be idempotent and must stop the callback.
    fn close(&mut self);
}

/// A MIDI input backend.
pub trait MidiInputBackend {
    /// Enumerate currently connected input ports.
    fn list_ports(&self) -> Result<Vec<MidiPortInfo>, MidiError>;

    /// Open `port` and deliver every message to `handler`.
    fn open(
        &mut self,
        port: &MidiPortInfo,
        handler: MidiHandler,
    ) -> Result<Box<dyn MidiConnection>, MidiError>;
}

struct FakeShared {
    handler: Mutex<Option<MidiHandler>>,
    open: AtomicBool,
    closes: Arc<AtomicUsize>,
}

impl FakeShared {
    fn close(&self) {
        if self.open.swap(false, Ordering::SeqCst) {
            *self.handler.lock().expect("fake handler lock") = None;
            self.closes.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Scriptable backend used by tests and the latency harness.
///
/// `deliver` calls the open connection's handler synchronously, on the calling
/// thread, exactly like a driver callback would.
pub struct FakeMidiBackend {
    ports: Vec<MidiPortInfo>,
    endpoints: Vec<(MidiPortInfo, Arc<FakeShared>)>,
    fail_open_for: Option<String>,
    opens: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
}

impl FakeMidiBackend {
    /// Backend with the given port names.
    #[must_use]
    pub fn new(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let ports = names
            .into_iter()
            .enumerate()
            .map(|(index, name)| MidiPortInfo::new(index, name))
            .collect();
        Self {
            ports,
            endpoints: Vec::new(),
            fail_open_for: None,
            opens: Arc::new(AtomicUsize::new(0)),
            closes: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Replace the enumerated port list, simulating an unplug or replug.
    pub fn set_ports(&mut self, names: impl IntoIterator<Item = impl Into<String>>) {
        self.ports = names
            .into_iter()
            .enumerate()
            .map(|(index, name)| MidiPortInfo::new(index, name))
            .collect();
    }

    /// Force the next open of `name` to fail.
    pub fn fail_next_open(&mut self, name: impl Into<String>) {
        self.fail_open_for = Some(name.into());
    }

    /// Deliver `bytes` to the open connection for `port_name`.
    ///
    /// Returns true when a connection received the message.
    pub fn deliver(&self, port_name: &str, timestamp_us: u64, bytes: &[u8]) -> bool {
        for (port, shared) in &self.endpoints {
            if port.name == port_name && shared.open.load(Ordering::SeqCst) {
                let mut guard = shared.handler.lock().expect("fake handler lock");
                if let Some(handler) = guard.as_mut() {
                    handler(timestamp_us, bytes);
                    return true;
                }
            }
        }
        false
    }

    /// Number of successful opens since creation.
    #[must_use]
    pub fn open_count(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    /// Number of closes since creation.
    #[must_use]
    pub fn close_count(&self) -> usize {
        self.closes.load(Ordering::SeqCst)
    }

    /// Names of ports with a live connection.
    #[must_use]
    pub fn open_ports(&self) -> Vec<String> {
        self.endpoints
            .iter()
            .filter(|(_, shared)| shared.open.load(Ordering::SeqCst))
            .map(|(port, _)| port.name.clone())
            .collect()
    }
}

struct FakeConnection {
    port: MidiPortInfo,
    shared: Arc<FakeShared>,
}

impl MidiConnection for FakeConnection {
    fn port(&self) -> &MidiPortInfo {
        &self.port
    }

    fn close(&mut self) {
        self.shared.close();
    }
}

impl Drop for FakeConnection {
    fn drop(&mut self) {
        self.shared.close();
    }
}

impl MidiInputBackend for FakeMidiBackend {
    fn list_ports(&self) -> Result<Vec<MidiPortInfo>, MidiError> {
        Ok(self.ports.clone())
    }

    fn open(
        &mut self,
        port: &MidiPortInfo,
        handler: MidiHandler,
    ) -> Result<Box<dyn MidiConnection>, MidiError> {
        let current = self
            .ports
            .iter()
            .find(|candidate| candidate.index == port.index && candidate.name == port.name)
            .ok_or_else(|| MidiError::PortNotFound {
                name: port.name.clone(),
            })?;
        if self.fail_open_for.as_deref() == Some(port.name.as_str()) {
            self.fail_open_for = None;
            return Err(MidiError::OpenFailed {
                name: port.name.clone(),
                reason: "fake failure".to_owned(),
            });
        }
        let shared = Arc::new(FakeShared {
            handler: Mutex::new(Some(handler)),
            open: AtomicBool::new(true),
            closes: Arc::clone(&self.closes),
        });
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.endpoints
            .retain(|(existing, _)| existing.name != current.name);
        self.endpoints.push((current.clone(), Arc::clone(&shared)));
        Ok(Box::new(FakeConnection {
            port: current.clone(),
            shared,
        }))
    }
}

#[cfg(feature = "midir-backend")]
mod midir_backend {
    use super::{MidiConnection, MidiError, MidiHandler, MidiInputBackend, MidiPortInfo};
    use midir::{Ignore, MidiInput};

    /// Real USB-MIDI input through midir.
    #[derive(Debug, Default)]
    pub struct MidirBackend;

    impl MidirBackend {
        /// Create a backend. Ports are enumerated lazily so a missing MIDI
        /// service only fails when the user actually asks.
        #[must_use]
        pub fn new() -> Self {
            Self
        }

        fn input() -> Result<MidiInput, MidiError> {
            let mut input = MidiInput::new("Drumpad")
                .map_err(|error| MidiError::Unavailable(error.to_string()))?;
            input.ignore(Ignore::None);
            Ok(input)
        }
    }

    struct MidirConnection {
        port: MidiPortInfo,
        connection: Option<midir::MidiInputConnection<()>>,
    }

    impl MidiConnection for MidirConnection {
        fn port(&self) -> &MidiPortInfo {
            &self.port
        }

        fn close(&mut self) {
            if let Some(connection) = self.connection.take() {
                connection.close();
            }
        }
    }

    impl Drop for MidirConnection {
        fn drop(&mut self) {
            self.close();
        }
    }

    impl MidiInputBackend for MidirBackend {
        fn list_ports(&self) -> Result<Vec<MidiPortInfo>, MidiError> {
            let input = Self::input()?;
            let ports = input.ports();
            let mut listed = Vec::with_capacity(ports.len());
            for (index, port) in ports.iter().enumerate() {
                let name = input
                    .port_name(port)
                    .unwrap_or_else(|_| format!("MIDI input {index}"));
                listed.push(MidiPortInfo {
                    index,
                    name,
                    id: Some(port.id()),
                });
            }
            Ok(listed)
        }

        fn open(
            &mut self,
            port: &MidiPortInfo,
            mut handler: MidiHandler,
        ) -> Result<Box<dyn MidiConnection>, MidiError> {
            let input = Self::input()?;
            let ports = input.ports();
            let target = ports
                .iter()
                .find(|candidate| {
                    input.port_name(candidate).ok().as_deref() == Some(port.name.as_str())
                })
                .or_else(|| ports.get(port.index))
                .ok_or_else(|| MidiError::PortNotFound {
                    name: port.name.clone(),
                })?;
            let connection = input
                .connect(
                    target,
                    "Drumpad input",
                    move |timestamp_us, message, _| handler(timestamp_us, message),
                    (),
                )
                .map_err(|error| MidiError::OpenFailed {
                    name: port.name.clone(),
                    reason: error.to_string(),
                })?;
            Ok(Box::new(MidirConnection {
                port: port.clone(),
                connection: Some(connection),
            }))
        }
    }
}

#[cfg(feature = "midir-backend")]
pub use midir_backend::MidirBackend;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn fake_backend_lists_and_opens_ports() {
        let mut backend = FakeMidiBackend::new(["Nitro Max", "Virtual Piano"]);
        assert_eq!(backend.list_ports().unwrap().len(), 2);
        let port = backend.list_ports().unwrap()[0].clone();
        let received = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&received);
        let connection = backend
            .open(
                &port,
                Box::new(move |_timestamp, _bytes| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .unwrap();
        assert_eq!(connection.port().name, "Nitro Max");
        assert!(backend.deliver("Nitro Max", 1, &[0x90, 36, 100]));
        assert_eq!(received.load(Ordering::SeqCst), 1);
        assert_eq!(backend.open_count(), 1);
        assert_eq!(backend.open_ports(), vec!["Nitro Max"]);
    }

    #[test]
    fn dropping_a_connection_stops_delivery_and_counts_once() {
        let mut backend = FakeMidiBackend::new(["Nitro Max"]);
        let port = backend.list_ports().unwrap().remove(0);
        let connection = backend.open(&port, Box::new(|_, _| {})).unwrap();
        assert!(backend.deliver("Nitro Max", 1, &[0x90, 36, 100]));
        drop(connection);
        assert!(!backend.deliver("Nitro Max", 2, &[0x90, 36, 100]));
        assert_eq!(backend.close_count(), 1);
        assert!(backend.open_ports().is_empty());
    }

    #[test]
    fn explicit_close_is_idempotent() {
        let mut backend = FakeMidiBackend::new(["Nitro Max"]);
        let port = backend.list_ports().unwrap().remove(0);
        let mut connection = backend.open(&port, Box::new(|_, _| {})).unwrap();
        connection.close();
        connection.close();
        assert_eq!(backend.close_count(), 1);
    }

    #[test]
    fn missing_port_and_forced_failure_are_typed() {
        let mut backend = FakeMidiBackend::new(["Nitro Max"]);
        let missing = MidiPortInfo::new(7, "Ghost");
        assert!(matches!(
            backend.open(&missing, Box::new(|_, _| {})),
            Err(MidiError::PortNotFound { .. })
        ));
        let port = backend.list_ports().unwrap().remove(0);
        backend.fail_next_open("Nitro Max");
        assert!(matches!(
            backend.open(&port, Box::new(|_, _| {})),
            Err(MidiError::OpenFailed { .. })
        ));
        assert!(backend.open(&port, Box::new(|_, _| {})).is_ok());
    }

    #[test]
    fn replugging_replaces_the_old_callback() {
        let mut backend = FakeMidiBackend::new(["Nitro Max"]);
        let port = backend.list_ports().unwrap().remove(0);
        let first = Arc::new(AtomicUsize::new(0));
        let first_counter = Arc::clone(&first);
        let old = backend
            .open(
                &port,
                Box::new(move |_, _| {
                    first_counter.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .unwrap();
        let second = Arc::new(AtomicUsize::new(0));
        let second_counter = Arc::clone(&second);
        let new = backend
            .open(
                &port,
                Box::new(move |_, _| {
                    second_counter.fetch_add(1, Ordering::SeqCst);
                }),
            )
            .unwrap();
        assert!(backend.deliver("Nitro Max", 1, &[0x90, 36, 100]));
        assert_eq!(first.load(Ordering::SeqCst), 0);
        assert_eq!(second.load(Ordering::SeqCst), 1);
        drop(old);
        drop(new);
    }
}
