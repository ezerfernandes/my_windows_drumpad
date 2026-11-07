//! MIDI hotplug, reconnect, and persisted device selection.
//!
//! Windows MIDI ports appear and disappear without a callback, so the app
//! polls this manager from its UI tick (never from the audio thread). The
//! manager enumerates ports, closes a vanished connection, reconnects the
//! saved device by name, and reports transitions as [`HotplugEvent`] values.
//!
//! When a device disappears, the app must call
//! [`crate::midi::router::MidiRouter::release_all`] with the current timestamp
//! so no gated voice is stranded, then keep using the keyboard path. Both
//! paths are independent, so fallback needs no restart.

use serde::{Deserialize, Serialize};

use crate::midi::backend::{
    MidiConnection, MidiError, MidiHandler, MidiInputBackend, MidiPortInfo,
};
use crate::midi::parse::ChannelFilter;

const fn auto_reconnect_default() -> bool {
    true
}

/// Persisted MIDI preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiPreferences {
    /// Device name to reconnect on startup and hotplug.
    #[serde(default)]
    pub device_name: Option<String>,
    /// Channel filter for the device.
    #[serde(default)]
    pub channel_filter: ChannelFilter,
    /// Reopen the saved device automatically when it reappears.
    #[serde(default = "auto_reconnect_default")]
    pub auto_reconnect: bool,
}

impl Default for MidiPreferences {
    fn default() -> Self {
        Self {
            device_name: None,
            channel_filter: ChannelFilter::All,
            auto_reconnect: true,
        }
    }
}

/// A hotplug transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotplugEvent {
    /// A connection was opened to the given port.
    Connected {
        /// Port that was opened.
        port: MidiPortInfo,
    },
    /// The open port disappeared and its connection was closed.
    Disconnected {
        /// Port that vanished.
        port: MidiPortInfo,
    },
    /// The port returned but could not be reopened.
    ReconnectFailed {
        /// Port name.
        name: String,
        /// Backend failure.
        error: MidiError,
    },
    /// The enumerated port list changed.
    PortsChanged {
        /// Current ports.
        ports: Vec<MidiPortInfo>,
    },
}

/// Owns a backend and at most one live connection.
pub struct HotplugManager<B: MidiInputBackend> {
    backend: B,
    preferences: MidiPreferences,
    connection: Option<Box<dyn MidiConnection>>,
    last_ports: Vec<MidiPortInfo>,
    last_error: Option<MidiError>,
    handler_factory: Box<dyn Fn() -> MidiHandler + Send>,
}

impl<B: MidiInputBackend> HotplugManager<B> {
    /// Create a manager. The connection stays closed until [`Self::poll`] or
    /// [`Self::connect_selected`] runs.
    ///
    /// `handler_factory` is called for every open, so a failed reopen never
    /// consumes the callback the app needs for the next attempt.
    pub fn new(
        backend: B,
        preferences: MidiPreferences,
        handler_factory: impl Fn() -> MidiHandler + Send + 'static,
    ) -> Self {
        Self {
            backend,
            preferences,
            connection: None,
            last_ports: Vec::new(),
            last_error: None,
            handler_factory: Box::new(handler_factory),
        }
    }

    /// Active preferences.
    #[must_use]
    pub fn preferences(&self) -> &MidiPreferences {
        &self.preferences
    }

    /// Change the saved device. The next poll connects it when present.
    pub fn set_device_name(&mut self, name: Option<String>) {
        self.preferences.device_name = name;
    }

    /// Current connection port, if any.
    #[must_use]
    pub fn current_port(&self) -> Option<&MidiPortInfo> {
        self.connection.as_ref().map(|connection| connection.port())
    }

    /// True when a connection is open.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }

    /// Last backend failure.
    #[must_use]
    pub fn last_error(&self) -> Option<&MidiError> {
        self.last_error.as_ref()
    }

    /// Backend, for tests and diagnostics.
    #[must_use]
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Mutable backend, for tests that simulate unplug/replug.
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Open the saved device if it is present and nothing is connected.
    pub fn connect_selected(&mut self) -> Result<Option<HotplugEvent>, MidiError> {
        if self.connection.is_some() {
            return Ok(None);
        }
        let Some(name) = self.preferences.device_name.clone() else {
            return Ok(None);
        };
        let ports = self.backend.list_ports()?;
        let Some(port) = ports.into_iter().find(|port| port.name == name) else {
            return Err(MidiError::PortNotFound { name });
        };
        self.open(port).map(Some)
    }

    /// Explicitly select and open a port, closing any previous connection.
    pub fn select(&mut self, port: &MidiPortInfo) -> Result<HotplugEvent, MidiError> {
        self.close_current();
        self.preferences.device_name = Some(port.name.clone());
        self.open(port.clone())
    }

    /// Poll for device changes and reconnect the saved device.
    ///
    /// Returns transitions in the order they should be applied.
    pub fn poll(&mut self) -> Result<Vec<HotplugEvent>, MidiError> {
        let ports = self.backend.list_ports()?;
        let mut events = Vec::new();

        let vanished = self
            .connection
            .as_ref()
            .map(|connection| connection.port().clone())
            .filter(|port| !ports.iter().any(|candidate| candidate.name == port.name));
        if let Some(port) = vanished {
            self.close_current();
            events.push(HotplugEvent::Disconnected { port });
        }

        let desired = self.preferences.device_name.clone();
        if self.connection.is_none() && self.preferences.auto_reconnect {
            if let Some(name) = desired {
                if let Some(port) = ports.iter().find(|port| port.name == name).cloned() {
                    match self.open(port) {
                        Ok(event) => events.push(event),
                        Err(error) => {
                            self.last_error = Some(error.clone());
                            events.push(HotplugEvent::ReconnectFailed { name, error });
                        }
                    }
                }
            }
        }

        if ports != self.last_ports {
            self.last_ports.clone_from(&ports);
            events.push(HotplugEvent::PortsChanged { ports });
        }

        Ok(events)
    }

    /// Close the current connection without clearing the saved device.
    pub fn disconnect(&mut self) -> Option<HotplugEvent> {
        let port = self
            .connection
            .as_ref()
            .map(|connection| connection.port().clone())?;
        self.close_current();
        Some(HotplugEvent::Disconnected { port })
    }

    fn open(&mut self, port: MidiPortInfo) -> Result<HotplugEvent, MidiError> {
        let handler = (self.handler_factory)();
        match self.backend.open(&port, handler) {
            Ok(connection) => {
                self.connection = Some(connection);
                self.last_error = None;
                Ok(HotplugEvent::Connected { port })
            }
            Err(error) => {
                self.last_error = Some(error.clone());
                Err(error)
            }
        }
    }

    fn close_current(&mut self) {
        if let Some(mut connection) = self.connection.take() {
            connection.close();
        }
    }
}

impl<B: MidiInputBackend> Drop for HotplugManager<B> {
    fn drop(&mut self) {
        self.close_current();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::backend::FakeMidiBackend;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn manager<'a>(
        names: impl IntoIterator<Item = &'a str>,
        preferences: MidiPreferences,
        counter: Option<Arc<AtomicUsize>>,
    ) -> HotplugManager<FakeMidiBackend> {
        let factory = move || {
            let counter = counter.clone();
            let handler: MidiHandler = match counter {
                Some(counter) => Box::new(move |_, _| {
                    counter.fetch_add(1, Ordering::SeqCst);
                }),
                None => Box::new(|_, _| {}),
            };
            handler
        };
        HotplugManager::new(FakeMidiBackend::new(names), preferences, factory)
    }

    #[test]
    fn saved_device_reconnects_on_poll() {
        let preferences = MidiPreferences {
            device_name: Some("Nitro Max".to_owned()),
            ..MidiPreferences::default()
        };
        let mut manager = manager(["Nitro Max"], preferences, None);
        let events = manager.poll().unwrap();
        assert!(matches!(
            events.first(),
            Some(HotplugEvent::Connected { port }) if port.name == "Nitro Max"
        ));
        assert!(manager.is_connected());
        assert_eq!(manager.current_port().unwrap().name, "Nitro Max");
    }

    #[test]
    fn repeated_polls_do_not_open_twice() {
        let preferences = MidiPreferences {
            device_name: Some("Nitro Max".to_owned()),
            ..MidiPreferences::default()
        };
        let mut manager = manager(["Nitro Max"], preferences, None);
        manager.poll().unwrap();
        manager.poll().unwrap();
        manager.poll().unwrap();
        assert_eq!(manager.backend().open_count(), 1);
    }

    #[test]
    fn unplug_closes_the_connection_and_replug_reopens_it() {
        let preferences = MidiPreferences {
            device_name: Some("Nitro Max".to_owned()),
            ..MidiPreferences::default()
        };
        let mut manager = manager(["Nitro Max"], preferences, None);
        manager.poll().unwrap();
        manager.backend_mut().set_ports(Vec::<String>::new());
        let events = manager.poll().unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, HotplugEvent::Disconnected { .. }))
        );
        assert!(!manager.is_connected());
        assert_eq!(manager.backend().close_count(), 1);

        manager.backend_mut().set_ports(["Nitro Max"]);
        let events = manager.poll().unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, HotplugEvent::Connected { .. }))
        );
        assert!(manager.is_connected());
        assert_eq!(manager.backend().open_count(), 2);
        assert_eq!(manager.backend().open_ports(), vec!["Nitro Max"]);
    }

    #[test]
    fn missing_saved_device_is_reported_without_opening() {
        let preferences = MidiPreferences {
            device_name: Some("Absent".to_owned()),
            ..MidiPreferences::default()
        };
        let mut manager = manager(["Other"], preferences, None);
        let events = manager.poll().unwrap();
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, HotplugEvent::Connected { .. }))
        );
        assert!(!manager.is_connected());
        assert!(matches!(
            manager.connect_selected(),
            Err(MidiError::PortNotFound { .. })
        ));
    }

    #[test]
    fn select_switches_devices_and_persists_the_name() {
        let mut manager = manager(["A", "B"], MidiPreferences::default(), None);
        let ports = manager.backend().list_ports().unwrap();
        let first = manager.select(&ports[0]).unwrap();
        assert!(matches!(first, HotplugEvent::Connected { .. }));
        assert_eq!(manager.preferences().device_name.as_deref(), Some("A"));
        let second = manager.select(&ports[1]).unwrap();
        assert!(matches!(second, HotplugEvent::Connected { .. }));
        assert_eq!(manager.preferences().device_name.as_deref(), Some("B"));
        assert_eq!(manager.backend().open_count(), 2);
        assert_eq!(manager.backend().close_count(), 1);
    }

    #[test]
    fn failed_reconnect_is_typed() {
        let preferences = MidiPreferences {
            device_name: Some("Nitro Max".to_owned()),
            ..MidiPreferences::default()
        };
        let mut manager = manager(["Nitro Max"], preferences, None);
        manager.backend_mut().fail_next_open("Nitro Max");
        let events = manager.poll().unwrap();
        assert!(matches!(
            events.first(),
            Some(HotplugEvent::ReconnectFailed { name, .. }) if name == "Nitro Max"
        ));
        assert!(manager.last_error().is_some());
        assert!(!manager.is_connected());
        // A later poll recovers.
        manager.poll().unwrap();
        assert!(manager.is_connected());
    }

    #[test]
    fn handler_receives_delivered_messages() {
        let counter = Arc::new(AtomicUsize::new(0));
        let preferences = MidiPreferences {
            device_name: Some("Nitro Max".to_owned()),
            ..MidiPreferences::default()
        };
        let mut manager = manager(["Nitro Max"], preferences, Some(Arc::clone(&counter)));
        manager.poll().unwrap();
        manager.backend().deliver("Nitro Max", 1, &[0x90, 36, 100]);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        manager.disconnect();
        manager.backend().deliver("Nitro Max", 2, &[0x90, 36, 100]);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn selected_device_survives_a_restart() {
        let preferences = MidiPreferences::default();
        let mut first = manager(["Alesis Nitro Max"], preferences, None);
        let ports = first.backend().list_ports().unwrap();
        first.select(&ports[0]).unwrap();
        let saved = first.preferences().clone();
        drop(first);

        // A new process starts with the persisted preferences.
        let mut restarted = manager(["Alesis Nitro Max"], saved, None);
        let events = restarted.poll().unwrap();
        assert!(matches!(
            events.first(),
            Some(HotplugEvent::Connected { port }) if port.name == "Alesis Nitro Max"
        ));
        assert_eq!(restarted.backend().open_count(), 1);
    }

    #[test]
    fn preferences_round_trip_through_json() {
        let preferences = MidiPreferences {
            device_name: Some("Alesis Nitro Max".to_owned()),
            channel_filter: ChannelFilter::Single(9),
            auto_reconnect: false,
        };
        let json = serde_json::to_string(&preferences).unwrap();
        assert_eq!(
            serde_json::from_str::<MidiPreferences>(&json).unwrap(),
            preferences
        );
        let defaults = MidiPreferences::default();
        assert!(defaults.auto_reconnect);
    }
}
