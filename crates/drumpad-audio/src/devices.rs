//! Render-device enumeration and hotplug watching.
//!
//! Device identity is the Windows endpoint id (`{0.0.0...}.{GUID}`), which is
//! stable across reboots. The friendly name is kept as a human-readable
//! fallback when an id is not available.

use crate::AudioError;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Endpoint state as reported by Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    /// Present and usable.
    Active,
    /// Disabled in the Windows sound control panel.
    Disabled,
    /// The adapter or endpoint was removed.
    NotPresent,
    /// Jack-presence detection reports nothing plugged in.
    Unplugged,
    /// The platform did not report a state.
    Unknown,
}

impl DeviceState {
    /// True when a stream can be opened on the device.
    pub fn is_usable(self) -> bool {
        matches!(self, DeviceState::Active)
    }
}

/// One render endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable endpoint id.
    pub id: String,
    /// Friendly name shown to the user.
    pub name: String,
    /// Whether this is the current default render endpoint.
    pub is_default: bool,
    /// Current endpoint state.
    pub state: DeviceState,
}

/// Source of render-device lists. The Windows implementation uses WASAPI; the
/// fake implementation lets tests drive hotplug deterministically.
pub trait DeviceProvider: Send + Sync {
    /// List render endpoints, default first.
    fn list_render_devices(&self) -> Result<Vec<DeviceInfo>, AudioError>;
}

/// Live WASAPI enumeration.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsDeviceProvider;

impl DeviceProvider for WindowsDeviceProvider {
    fn list_render_devices(&self) -> Result<Vec<DeviceInfo>, AudioError> {
        windows_devices()
    }
}

/// A test provider whose device list can be mutated from outside.
#[derive(Debug, Clone, Default)]
pub struct FakeDeviceProvider {
    devices: Arc<Mutex<Vec<DeviceInfo>>>,
}

impl FakeDeviceProvider {
    /// Create a provider with an initial device list.
    pub fn new(devices: Vec<DeviceInfo>) -> Self {
        Self {
            devices: Arc::new(Mutex::new(devices)),
        }
    }

    /// Replace the device list.
    pub fn set_devices(&self, devices: Vec<DeviceInfo>) {
        *self.devices.lock().unwrap() = devices;
    }

    /// Snapshot the current list.
    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.devices.lock().unwrap().clone()
    }
}

impl DeviceProvider for FakeDeviceProvider {
    fn list_render_devices(&self) -> Result<Vec<DeviceInfo>, AudioError> {
        Ok(self.devices())
    }
}

/// A device hotplug notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    /// A new endpoint appeared.
    Added(DeviceInfo),
    /// An endpoint disappeared.
    Removed(String),
    /// An endpoint changed state or name.
    Changed(DeviceInfo),
    /// The default render endpoint changed.
    DefaultChanged(String),
    /// Enumeration failed; the watcher keeps polling.
    Error(String),
}

/// Polls a [`DeviceProvider`] and emits hotplug events.
///
/// Polling every 500 ms gives a worst-case two-second recovery window across
/// unplug, reconnect and reopen, matching the device-loss acceptance target.
pub struct DeviceWatcher {
    stop: Arc<AtomicBool>,
    baseline_ready: Arc<AtomicBool>,
    receiver: mpsc::Receiver<DeviceEvent>,
    handle: Option<JoinHandle<()>>,
    interval: Duration,
}

impl DeviceWatcher {
    /// Spawn a watcher. Polling starts immediately.
    pub fn spawn(provider: Box<dyn DeviceProvider>, interval: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let baseline_ready = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let thread_stop = Arc::clone(&stop);
        let thread_ready = Arc::clone(&baseline_ready);
        let interval = interval.max(Duration::from_millis(20));
        let handle = thread::spawn(move || {
            let mut previous: BTreeMap<String, DeviceInfo> = BTreeMap::new();
            let mut previous_default: Option<String> = None;
            let mut initialized = false;
            while !thread_stop.load(Ordering::Relaxed) {
                match provider.list_render_devices() {
                    Ok(devices) => {
                        let mut current = BTreeMap::new();
                        let mut current_default = None;
                        for device in devices {
                            if device.is_default {
                                current_default = Some(device.id.clone());
                            }
                            if initialized {
                                match previous.get(&device.id) {
                                    None => {
                                        let _ = sender.send(DeviceEvent::Added(device.clone()));
                                    }
                                    Some(old) if old != &device => {
                                        let _ = sender.send(DeviceEvent::Changed(device.clone()));
                                    }
                                    _ => {}
                                }
                            }
                            current.insert(device.id.clone(), device);
                        }
                        if initialized {
                            for id in previous.keys() {
                                if !current.contains_key(id) {
                                    let _ = sender.send(DeviceEvent::Removed(id.clone()));
                                }
                            }
                            if current_default != previous_default
                                && let Some(id) = current_default.clone()
                            {
                                let _ = sender.send(DeviceEvent::DefaultChanged(id));
                            }
                        }
                        previous_default = current_default;
                        previous = current;
                        initialized = true;
                        thread_ready.store(true, Ordering::Relaxed);
                    }
                    Err(error) => {
                        let _ = sender.send(DeviceEvent::Error(error.to_string()));
                    }
                }
                // Sleep in small slices so stop() is responsive.
                let deadline = std::time::Instant::now() + interval;
                while std::time::Instant::now() < deadline {
                    if thread_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10).min(interval));
                }
            }
        });
        Self {
            stop,
            baseline_ready,
            receiver,
            handle: Some(handle),
            interval,
        }
    }

    /// True once the first device snapshot has been taken. Changes made before
    /// this point become the baseline and produce no events.
    pub fn is_baseline_ready(&self) -> bool {
        self.baseline_ready.load(Ordering::Relaxed)
    }

    /// Wait until the first snapshot is ready, up to `timeout`.
    pub fn wait_for_baseline(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self.is_baseline_ready() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        self.is_baseline_ready()
    }

    /// Receive one event without blocking.
    pub fn try_recv(&self) -> Option<DeviceEvent> {
        self.receiver.try_recv().ok()
    }

    /// Receive one event, waiting up to `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<DeviceEvent> {
        self.receiver.recv_timeout(timeout).ok()
    }

    /// Polling interval.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Ask the watcher to stop and join it.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for DeviceWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

/// List render devices with the live WASAPI provider.
pub fn list_render_devices() -> Result<Vec<DeviceInfo>, AudioError> {
    WindowsDeviceProvider.list_render_devices()
}

/// Pick the default render device, if one exists.
pub fn default_render_device() -> Result<Option<DeviceInfo>, AudioError> {
    Ok(list_render_devices()?.into_iter().find(|d| d.is_default))
}

#[cfg(windows)]
fn windows_devices() -> Result<Vec<DeviceInfo>, AudioError> {
    use wasapi::{DeviceEnumerator, Direction};

    ensure_com();
    let enumerator = DeviceEnumerator::new()?;
    let default_id = enumerator
        .get_default_device(Direction::Render)
        .ok()
        .and_then(|device| device.get_id().ok());
    let collection = enumerator.get_device_collection(Direction::Render)?;
    let count = collection.get_nbr_devices()?;
    let mut devices = Vec::with_capacity(count as usize);
    for index in 0..count {
        let Ok(device) = collection.get_device_at_index(index) else {
            continue;
        };
        let Ok(id) = device.get_id() else {
            continue;
        };
        let name = device
            .get_friendlyname()
            .unwrap_or_else(|_| "<unnamed audio device>".to_string());
        let state = device
            .get_state()
            .map(map_device_state)
            .unwrap_or(DeviceState::Unknown);
        let is_default = default_id.as_deref() == Some(id.as_str());
        devices.push(DeviceInfo {
            id,
            name,
            is_default,
            state,
        });
    }
    devices.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.state.is_usable().cmp(&b.state.is_usable()).reverse())
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(devices)
}

#[cfg(not(windows))]
fn windows_devices() -> Result<Vec<DeviceInfo>, AudioError> {
    Err(AudioError::Backend(
        "WASAPI device enumeration is only available on Windows".into(),
    ))
}

#[cfg(windows)]
fn map_device_state(state: wasapi::DeviceState) -> DeviceState {
    match state {
        wasapi::DeviceState::Active => DeviceState::Active,
        wasapi::DeviceState::Disabled => DeviceState::Disabled,
        wasapi::DeviceState::NotPresent => DeviceState::NotPresent,
        wasapi::DeviceState::Unplugged => DeviceState::Unplugged,
    }
}

/// Initialize COM for the calling thread. WASAPI needs either apartment; use
/// MTA first and fall back to STA on a UI thread that already owns an STA.
#[cfg(windows)]
pub(crate) fn ensure_com() {
    if wasapi::initialize_mta().is_err() {
        let _ = wasapi::initialize_sta();
    }
}

#[cfg(not(windows))]
pub(crate) fn ensure_com() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, name: &str, is_default: bool) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            name: name.into(),
            is_default,
            state: DeviceState::Active,
        }
    }

    #[test]
    fn watcher_emits_add_remove_and_default_change() {
        let provider = FakeDeviceProvider::new(vec![device("a", "Speakers A", true)]);
        let mut watcher =
            DeviceWatcher::spawn(Box::new(provider.clone()), Duration::from_millis(20));
        assert!(watcher.wait_for_baseline(Duration::from_millis(1000)));
        // The initial list is the baseline: no events for it.
        assert!(watcher.recv_timeout(Duration::from_millis(100)).is_none());

        provider.set_devices(vec![
            device("a", "Speakers A", true),
            device("b", "Headset", false),
        ]);
        let event = watcher.recv_timeout(Duration::from_millis(1000));
        assert!(matches!(event, Some(DeviceEvent::Added(d)) if d.id == "b"));

        provider.set_devices(vec![device("b", "Headset", true)]);
        let mut removed = None;
        for _ in 0..8 {
            match watcher.recv_timeout(Duration::from_millis(500)) {
                Some(DeviceEvent::Removed(id)) => {
                    removed = Some(id);
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert_eq!(removed.as_deref(), Some("a"));

        watcher.stop();
    }

    #[test]
    fn watcher_reports_default_change() {
        let provider = FakeDeviceProvider::new(vec![
            device("a", "Speakers A", true),
            device("b", "Headset", false),
        ]);
        let mut watcher =
            DeviceWatcher::spawn(Box::new(provider.clone()), Duration::from_millis(20));
        assert!(watcher.wait_for_baseline(Duration::from_millis(1000)));
        provider.set_devices(vec![
            device("a", "Speakers A", false),
            device("b", "Headset", true),
        ]);
        let mut default_change = None;
        for _ in 0..8 {
            match watcher.recv_timeout(Duration::from_millis(500)) {
                Some(DeviceEvent::DefaultChanged(id)) => {
                    default_change = Some(id);
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert_eq!(default_change.as_deref(), Some("b"));
        watcher.stop();
    }

    #[test]
    fn device_states_report_usability() {
        assert!(DeviceState::Active.is_usable());
        for state in [
            DeviceState::Disabled,
            DeviceState::NotPresent,
            DeviceState::Unplugged,
            DeviceState::Unknown,
        ] {
            assert!(!state.is_usable(), "{state:?} must not be usable");
        }
    }

    #[test]
    fn fake_provider_round_trips_its_device_list() {
        let provider = FakeDeviceProvider::new(vec![device("a", "Speakers", true)]);
        assert_eq!(provider.devices().len(), 1);
        provider.set_devices(vec![device("b", "Headset", false)]);
        let devices = provider.list_render_devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "b");
    }

    #[test]
    fn watcher_exposes_interval_and_try_recv() {
        let provider = FakeDeviceProvider::new(vec![device("a", "Speakers", true)]);
        let mut watcher = DeviceWatcher::spawn(Box::new(provider), Duration::from_millis(25));
        assert_eq!(watcher.interval(), Duration::from_millis(25));
        assert!(watcher.wait_for_baseline(Duration::from_millis(1000)));
        assert!(watcher.try_recv().is_none());
        watcher.stop();
        watcher.stop(); // idempotent
    }

    #[test]
    fn watcher_reports_provider_errors() {
        struct FailingProvider;
        impl DeviceProvider for FailingProvider {
            fn list_render_devices(&self) -> Result<Vec<DeviceInfo>, AudioError> {
                Err(AudioError::Backend("no audio service".into()))
            }
        }

        let mut watcher =
            DeviceWatcher::spawn(Box::new(FailingProvider), Duration::from_millis(20));
        let event = watcher.recv_timeout(Duration::from_millis(1000));
        assert!(
            matches!(event, Some(DeviceEvent::Error(message)) if message.contains("no audio service"))
        );
        watcher.stop();
    }

    #[test]
    fn watcher_reports_device_state_changes() {
        let provider = FakeDeviceProvider::new(vec![device("a", "Speakers", true)]);
        let mut watcher =
            DeviceWatcher::spawn(Box::new(provider.clone()), Duration::from_millis(20));
        assert!(watcher.wait_for_baseline(Duration::from_millis(1000)));

        let mut unplugged = device("a", "Speakers", true);
        unplugged.state = DeviceState::Unplugged;
        provider.set_devices(vec![unplugged]);

        let mut changed = None;
        for _ in 0..8 {
            match watcher.recv_timeout(Duration::from_millis(500)) {
                Some(DeviceEvent::Changed(info)) => {
                    changed = Some(info);
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert_eq!(changed.map(|info| info.state), Some(DeviceState::Unplugged));
        watcher.stop();
    }
}
