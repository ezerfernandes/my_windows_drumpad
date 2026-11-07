//! Application-facing audio system: settings, device resolution, stream
//! ownership, and hotplug recovery.
//!
//! The UI calls [`AudioSystem::tick`] a few times per second. If the stream
//! reports a device loss, the system closes it, polls for a usable endpoint,
//! and reopens the stream with a fresh callback from the factory. Recovery is
//! bounded by `reconnect_interval` + one device poll, well under the two-second
//! device-loss target.

use crate::AudioError;
use crate::backend::{AudioBackend, AudioCallback};
use crate::devices::DeviceInfo;
use crate::settings::{self, AudioSettings, SettingsLoad};
use crate::{RenderStream, StreamConfig, StreamFormat, StreamSnapshot, StreamState};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Factory invoked whenever a stream is (re)opened. Device loss destroys the
/// old callback, so the app must be able to build a fresh one (typically a new
/// engine wired to the same sample bank).
pub type CallbackFactory = Box<dyn FnMut() -> Box<dyn AudioCallback> + Send>;

/// High-level state of the audio system.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioSystemState {
    /// No stream and no recovery attempt running.
    Stopped,
    /// A stream is open.
    Running {
        /// Negotiated stream format.
        format: StreamFormat,
    },
    /// The stream was lost and the system is polling / retrying.
    Reconnecting {
        /// Why the stream stopped.
        reason: String,
    },
    /// The initial open failed; the app must retry explicitly.
    Failed {
        /// Failure reason.
        reason: String,
    },
}

impl AudioSystemState {
    /// True while audio is rendering.
    pub fn is_running(&self) -> bool {
        matches!(self, AudioSystemState::Running { .. })
    }
}

/// Events the UI should surface.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioSystemEvent {
    /// The first stream opened.
    Started(StreamFormat),
    /// A lost stream reopened.
    Reconnected(StreamFormat),
    /// The stream stopped because the device disappeared.
    DeviceLost(String),
    /// The requested device was unavailable and another one was used.
    DeviceFallback {
        /// Requested device id or name.
        requested: Option<String>,
        /// Device actually used.
        used: String,
    },
    /// No usable device exists; recovery keeps polling.
    Disconnected(String),
    /// Opening failed for a reason other than device loss.
    Failed(String),
}

/// Owns settings, the backend, and the current stream.
pub struct AudioSystem {
    backend: Box<dyn AudioBackend>,
    settings_path: Option<PathBuf>,
    settings: AudioSettings,
    callback_factory: CallbackFactory,
    stream: Option<RenderStream>,
    state: AudioSystemState,
    events: Vec<AudioSystemEvent>,
    reconnect_interval: Duration,
    last_attempt: Option<Instant>,
    last_disconnect_notice: Option<String>,
}

impl AudioSystem {
    /// Create a system. Call [`AudioSystem::load_settings`] before `start` when
    /// a settings path is configured.
    pub fn new(
        backend: Box<dyn AudioBackend>,
        settings_path: Option<PathBuf>,
        callback_factory: CallbackFactory,
    ) -> Self {
        Self {
            backend,
            settings_path,
            settings: AudioSettings::default(),
            callback_factory,
            stream: None,
            state: AudioSystemState::Stopped,
            events: Vec::new(),
            reconnect_interval: Duration::from_millis(250),
            last_attempt: None,
            last_disconnect_notice: None,
        }
    }

    /// Load persisted settings (or defaults) without starting a stream.
    pub fn load_settings(&mut self) -> SettingsLoad {
        let load = match &self.settings_path {
            Some(path) => settings::load(path),
            None => SettingsLoad {
                settings: AudioSettings::default(),
                warnings: Vec::new(),
                used_defaults: true,
            },
        };
        self.settings = load.settings.clone();
        load
    }

    /// Current settings.
    pub fn settings(&self) -> &AudioSettings {
        &self.settings
    }

    /// Replace settings. They take effect on the next (re)open.
    pub fn set_settings(&mut self, mut settings: AudioSettings) {
        settings.validate();
        self.settings = settings;
    }

    /// Current system state.
    pub fn state(&self) -> &AudioSystemState {
        &self.state
    }

    /// Drain UI events since the last call.
    pub fn drain_events(&mut self) -> Vec<AudioSystemEvent> {
        std::mem::take(&mut self.events)
    }

    /// Latest stream diagnostics, if a stream exists.
    pub fn snapshot(&self) -> Option<StreamSnapshot> {
        self.stream
            .as_ref()
            .map(|stream| stream.diagnostics().snapshot())
    }

    /// Negotiated format, if running.
    pub fn format(&self) -> Option<&StreamFormat> {
        self.stream.as_ref().map(|stream| stream.format())
    }

    /// Enumerate devices through the backend.
    pub fn devices(&self) -> Result<Vec<DeviceInfo>, AudioError> {
        self.backend.list_devices()
    }

    /// How often recovery retries, in seconds.
    pub fn reconnect_interval(&self) -> Duration {
        self.reconnect_interval
    }

    /// Set the recovery retry interval. Tests use a short value.
    pub fn set_reconnect_interval(&mut self, interval: Duration) {
        self.reconnect_interval = interval.max(Duration::from_millis(20));
    }

    /// Open the stream with the current settings.
    pub fn start(&mut self) -> Result<StreamFormat, AudioError> {
        self.stop_stream();
        match self.try_open() {
            Ok(format) => {
                self.state = AudioSystemState::Running {
                    format: format.clone(),
                };
                self.events.push(AudioSystemEvent::Started(format.clone()));
                self.persist_settings();
                Ok(format)
            }
            Err(error) => {
                self.state = AudioSystemState::Failed {
                    reason: error.to_string(),
                };
                self.events
                    .push(AudioSystemEvent::Failed(error.to_string()));
                Err(error)
            }
        }
    }

    /// Poll once. Call this from the UI at ~10 Hz.
    ///
    /// Returns the newest event produced by this tick (or `None`). Events
    /// remain queued for [`AudioSystem::drain_events`]; the return value is a
    /// convenience for callers that only care about the latest update.
    pub fn tick(&mut self) -> Option<AudioSystemEvent> {
        let before = self.events.len();
        self.check_running_stream();
        if matches!(self.state, AudioSystemState::Reconnecting { .. }) {
            self.attempt_reconnect();
        }
        if self.events.len() > before {
            self.events.last().cloned()
        } else {
            None
        }
    }

    /// Stop the stream and cancel recovery.
    pub fn stop(&mut self) {
        self.stop_stream();
        self.state = AudioSystemState::Stopped;
        self.last_attempt = None;
    }

    fn check_running_stream(&mut self) {
        let Some(stream) = self.stream.as_ref() else {
            return;
        };
        let snapshot = stream.diagnostics().snapshot();
        let lost = !stream.is_running()
            || matches!(
                snapshot.state,
                StreamState::DeviceLost | StreamState::Error | StreamState::Stopped
            );
        if !lost {
            return;
        }
        let reason = snapshot
            .error
            .unwrap_or_else(|| "stream stopped unexpectedly".into());
        self.stop_stream();
        self.last_attempt = None;
        self.state = AudioSystemState::Reconnecting {
            reason: reason.clone(),
        };
        self.events.push(AudioSystemEvent::DeviceLost(reason));
    }

    fn attempt_reconnect(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last_attempt
            && now.duration_since(last) < self.reconnect_interval
        {
            return;
        }
        self.last_attempt = Some(now);

        let devices = match self.backend.list_devices() {
            Ok(devices) => devices,
            Err(error) => {
                self.notify_disconnected(error.to_string());
                return;
            }
        };
        if let Err(error) = settings::resolve_device(&self.settings, &devices) {
            self.notify_disconnected(error.to_string());
            return;
        }
        match self.try_open() {
            Ok(format) => {
                self.last_disconnect_notice = None;
                self.state = AudioSystemState::Running {
                    format: format.clone(),
                };
                self.events.push(AudioSystemEvent::Reconnected(format));
                self.persist_settings();
            }
            Err(error) => {
                self.notify_disconnected(error.to_string());
            }
        }
    }

    fn notify_disconnected(&mut self, reason: String) {
        if self.last_disconnect_notice.as_deref() == Some(reason.as_str()) {
            return;
        }
        self.last_disconnect_notice = Some(reason.clone());
        self.events.push(AudioSystemEvent::Disconnected(reason));
    }

    fn try_open(&mut self) -> Result<StreamFormat, AudioError> {
        let devices = self.backend.list_devices()?;
        let (device, fell_back) = settings::resolve_device(&self.settings, &devices)?;
        if fell_back {
            self.events.push(AudioSystemEvent::DeviceFallback {
                requested: self
                    .settings
                    .device_id
                    .clone()
                    .or_else(|| self.settings.device_name.clone()),
                used: device.name.clone(),
            });
        }
        self.settings.device_id = Some(device.id.clone());
        self.settings.device_name = Some(device.name.clone());
        let config: StreamConfig = self.settings.stream_config();
        let callback = (self.callback_factory)();
        let stream = self.backend.open_stream(config, callback)?;
        let format = stream.format().clone();
        self.stream = Some(stream);
        Ok(format)
    }

    fn stop_stream(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            stream.stop();
        }
    }

    fn persist_settings(&self) {
        if let Some(path) = &self.settings_path {
            let _ = settings::save(path, &self.settings);
        }
    }
}

impl std::fmt::Debug for AudioSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioSystem")
            .field("backend", &self.backend.name())
            .field("state", &self.state)
            .field("settings", &self.settings)
            .field("stream", &self.stream)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::FakeBackend;
    use crate::diagnostics::StreamState;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct SilentCallback {
        opened: Arc<AtomicU32>,
    }

    impl AudioCallback for SilentCallback {
        fn process(&mut self, out: &mut [f32], frames: usize) {
            self.opened.fetch_add(1, Ordering::Relaxed);
            for sample in out.iter_mut().take(frames * 2) {
                *sample = 0.0;
            }
        }
    }

    fn factory(opened: Arc<AtomicU32>) -> CallbackFactory {
        Box::new(move || {
            Box::new(SilentCallback {
                opened: Arc::clone(&opened),
            })
        })
    }

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "drumpad-audio-system-{}-{}.json",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn starts_and_persists_settings() {
        let path = temp_path("start");
        let opened = Arc::new(AtomicU32::new(0));
        let backend = FakeBackend::standard();
        let mut system = AudioSystem::new(
            Box::new(backend),
            Some(path.clone()),
            factory(Arc::clone(&opened)),
        );
        let format = system.start().unwrap();
        assert_eq!(format.device_name, "Fake Speakers");
        assert!(system.state().is_running());
        let events = system.drain_events();
        assert!(matches!(events[0], AudioSystemEvent::Started(_)));
        let loaded = settings::load(&path);
        assert_eq!(loaded.settings.device_id.as_deref(), Some("fake-speakers"));
        system.stop();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reconnects_after_device_loss_within_two_seconds() {
        let opened = Arc::new(AtomicU32::new(0));
        let backend = FakeBackend::standard().with_period(Duration::from_millis(5));
        let failure = backend.clone();
        let mut system = AudioSystem::new(Box::new(backend), None, factory(Arc::clone(&opened)));
        system.set_reconnect_interval(Duration::from_millis(50));
        system.start().unwrap();
        system.drain_events();

        failure.set_stream_failure(true);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_lost = false;
        while Instant::now() < deadline && !saw_lost {
            system.tick();
            for event in system.drain_events() {
                if matches!(event, AudioSystemEvent::DeviceLost(_)) {
                    saw_lost = true;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(saw_lost, "device loss was not detected");
        assert!(matches!(
            system.state(),
            AudioSystemState::Reconnecting { .. }
        ));

        failure.set_stream_failure(false);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut reconnected = false;
        while Instant::now() < deadline && !reconnected {
            system.tick();
            for event in system.drain_events() {
                if matches!(event, AudioSystemEvent::Reconnected(_)) {
                    reconnected = true;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(reconnected, "system did not reconnect");
        assert!(system.state().is_running());
        system.stop();
    }

    #[test]
    fn reports_no_device_and_recovers_when_one_appears() {
        let opened = Arc::new(AtomicU32::new(0));
        let backend = FakeBackend::standard();
        let backend_clone = backend.clone();
        let mut system = AudioSystem::new(Box::new(backend), None, factory(opened));
        system.set_reconnect_interval(Duration::from_millis(50));
        // Start with no device.
        let devices = backend_clone.list_devices().unwrap();
        backend_clone.set_devices(Vec::new());
        let error = system.start().unwrap_err();
        assert!(matches!(error, AudioError::NoDevice));
        assert!(matches!(system.state(), AudioSystemState::Failed { .. }));

        // Make the system reconnect-capable by feeding a device back.
        backend_clone.set_devices(devices);
        // Failed is an explicit-retry state, so a fresh start succeeds.
        let format = system.start().unwrap();
        assert_eq!(format.device_name, "Fake Speakers");
        system.stop();
    }

    #[test]
    fn device_loss_stops_the_stream_and_leaves_a_clear_state() {
        let opened = Arc::new(AtomicU32::new(0));
        let backend = FakeBackend::standard().with_period(Duration::from_millis(5));
        let control = backend.clone();
        let mut system = AudioSystem::new(Box::new(backend), None, factory(opened));
        system.start().unwrap();
        control.set_stream_failure(true);
        std::thread::sleep(Duration::from_millis(30));
        system.tick();
        assert!(!system.state().is_running());
        assert!(system.snapshot().is_none());
        let events = system.drain_events();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AudioSystemEvent::DeviceLost(_)))
        );
        system.stop();
    }

    #[test]
    fn snapshot_reports_running_state() {
        let opened = Arc::new(AtomicU32::new(0));
        let backend = FakeBackend::standard().with_period(Duration::from_millis(5));
        let mut system = AudioSystem::new(Box::new(backend), None, factory(opened));
        system.start().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let snapshot = system.snapshot().unwrap();
        assert_eq!(snapshot.state, StreamState::Running);
        assert!(snapshot.callbacks > 0);
        system.stop();
    }
}
