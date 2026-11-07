//! Backend abstraction: device enumeration, capability reporting, and stream
//! opening. The Windows implementation lives behind this trait so the app and
//! tests never depend on WASAPI types.

use crate::AudioError;
use crate::devices::{DeviceInfo, DeviceProvider, DeviceState, WindowsDeviceProvider};
use crate::diagnostics::{StreamDiagnostics, StreamState};
use crate::stream::{
    RenderStream, SampleFormat, ShareMode, StreamConfig, StreamControl, StreamFormat,
    callback_interval, open_render_stream,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Audio callback invoked on the real-time stream thread.
///
/// Implementations must not allocate, lock, log, or touch the filesystem.
pub trait AudioCallback: Send + 'static {
    /// Fill `out` with `frames` interleaved stereo frames. The buffer is
    /// pre-filled with silence by the backend.
    fn process(&mut self, out: &mut [f32], frames: usize);
}

/// A configuration the device can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportedConfig {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
    /// Wire sample format.
    pub sample_format: SampleFormat,
    /// Sharing mode.
    pub share_mode: ShareMode,
    /// Smallest supported buffer/period in frames.
    pub min_buffer_frames: u32,
    /// Largest supported buffer in frames.
    pub max_buffer_frames: u32,
}

/// Platform audio backend.
pub trait AudioBackend: Send + Sync {
    /// Short backend name for diagnostics.
    fn name(&self) -> &'static str;

    /// List render endpoints, default first.
    fn list_devices(&self) -> Result<Vec<DeviceInfo>, AudioError>;

    /// List the configurations the device can run. Used by the settings UI to
    /// reject impossible combinations before opening a stream.
    fn supported_configs(&self, device_id: &str) -> Result<Vec<SupportedConfig>, AudioError>;

    /// Open and start a render stream. The returned handle owns the stream
    /// thread; dropping it stops the stream.
    fn open_stream(
        &self,
        config: StreamConfig,
        callback: Box<dyn AudioCallback>,
    ) -> Result<RenderStream, AudioError>;
}

/// Live WASAPI backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsBackend;

impl AudioBackend for WindowsBackend {
    fn name(&self) -> &'static str {
        "wasapi"
    }

    fn list_devices(&self) -> Result<Vec<DeviceInfo>, AudioError> {
        WindowsDeviceProvider.list_render_devices()
    }

    fn supported_configs(&self, device_id: &str) -> Result<Vec<SupportedConfig>, AudioError> {
        windows_supported_configs(device_id)
    }

    fn open_stream(
        &self,
        config: StreamConfig,
        callback: Box<dyn AudioCallback>,
    ) -> Result<RenderStream, AudioError> {
        open_render_stream(config, callback)
    }
}

#[cfg(windows)]
fn windows_supported_configs(device_id: &str) -> Result<Vec<SupportedConfig>, AudioError> {
    use wasapi::{CapabilityProbe, DeviceEnumerator, SampleType};

    crate::devices::ensure_com();
    let enumerator = DeviceEnumerator::new()?;
    let device = enumerator
        .get_device(device_id)
        .map_err(|_| AudioError::DeviceNotFound(device_id.to_string()))?;
    let client = device.get_iaudioclient()?;
    let (default_period, min_period) = client.get_device_period()?;
    let default_frames = (default_period as f64 * 48_000.0 / 10_000_000.0).round() as u32;
    let min_frames = (min_period as f64 * 48_000.0 / 10_000_000.0)
        .round()
        .max(16.0) as u32;
    let mut configs = Vec::new();

    if let Ok(mix) = client.get_mixformat() {
        configs.push(SupportedConfig {
            sample_rate: mix.get_samplespersec(),
            channels: mix.get_nchannels(),
            sample_format: SampleFormat::F32,
            share_mode: ShareMode::Shared,
            min_buffer_frames: default_frames.max(min_frames),
            max_buffer_frames: 4096,
        });
    }

    let mut probe = CapabilityProbe::new(&device)?;
    for format in probe.supported_formats_all_rates() {
        let rate = format.get_samplespersec();
        let channels = format.get_nchannels();
        if channels != 2 || !matches!(rate, 44_100 | 48_000 | 88_200 | 96_000 | 176_400 | 192_000) {
            continue;
        }
        let sample_format = match format.get_subformat() {
            Ok(SampleType::Float) => SampleFormat::F32,
            _ => match (format.get_bitspersample(), format.get_validbitspersample()) {
                (16, 16) => SampleFormat::I16,
                (24, 24) | (32, 24) => SampleFormat::I24,
                _ => continue,
            },
        };
        let config = SupportedConfig {
            sample_rate: rate,
            channels,
            sample_format,
            share_mode: ShareMode::Exclusive,
            min_buffer_frames: min_frames,
            max_buffer_frames: 4096,
        };
        if !configs.contains(&config) {
            configs.push(config);
        }
    }
    Ok(configs)
}

#[cfg(not(windows))]
fn windows_supported_configs(_device_id: &str) -> Result<Vec<SupportedConfig>, AudioError> {
    Err(AudioError::Backend(
        "WASAPI is only available on Windows".into(),
    ))
}

/// Deterministic backend for tests and offline runs.
///
/// `open_stream` starts a small thread that calls the callback at the
/// negotiated period, and [`FakeBackend::open_manual_stream`] returns a stream
/// the test drives block by block.
#[derive(Debug, Clone)]
pub struct FakeBackend {
    devices: Arc<std::sync::Mutex<Vec<DeviceInfo>>>,
    configs: Vec<SupportedConfig>,
    period: Duration,
    fail_open: Arc<AtomicBool>,
    stream_failure: Arc<AtomicBool>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::standard()
    }
}

impl FakeBackend {
    /// A backend with one active device and 48 kHz stereo support.
    pub fn standard() -> Self {
        let devices = vec![DeviceInfo {
            id: "fake-speakers".into(),
            name: "Fake Speakers".into(),
            is_default: true,
            state: DeviceState::Active,
        }];
        let configs = vec![
            SupportedConfig {
                sample_rate: 48_000,
                channels: 2,
                sample_format: SampleFormat::F32,
                share_mode: ShareMode::Shared,
                min_buffer_frames: 64,
                max_buffer_frames: 2048,
            },
            SupportedConfig {
                sample_rate: 48_000,
                channels: 2,
                sample_format: SampleFormat::I16,
                share_mode: ShareMode::Exclusive,
                min_buffer_frames: 64,
                max_buffer_frames: 2048,
            },
        ];
        Self {
            devices: Arc::new(std::sync::Mutex::new(devices)),
            configs,
            period: Duration::from_micros(500),
            fail_open: Arc::new(AtomicBool::new(false)),
            stream_failure: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Replace the device list (simulates hotplug).
    pub fn set_devices(&self, devices: Vec<DeviceInfo>) {
        *self.devices.lock().unwrap() = devices;
    }

    /// Force every running fake stream to report a device loss and stop.
    pub fn set_stream_failure(&self, fail: bool) {
        self.stream_failure.store(fail, Ordering::Relaxed);
    }

    /// Replace the supported configurations.
    pub fn with_configs(mut self, configs: Vec<SupportedConfig>) -> Self {
        self.configs = configs;
        self
    }

    /// Set the simulated thread period.
    pub fn with_period(mut self, period: Duration) -> Self {
        self.period = period.max(Duration::from_micros(50));
        self
    }

    /// Make every subsequent open fail.
    pub fn set_fail_open(&self, fail: bool) {
        self.fail_open.store(fail, Ordering::Relaxed);
    }

    /// Negotiate a format from this backend's capabilities.
    pub fn negotiate(&self, config: &StreamConfig) -> Result<StreamFormat, AudioError> {
        let device = self
            .device(&config.device_id)
            .ok_or_else(|| AudioError::DeviceNotFound(format!("{:?}", config.device_id)))?;
        if !device.state.is_usable() {
            return Err(AudioError::DeviceLost(device.id.clone()));
        }
        let exact = self.configs.iter().find(|supported| {
            supported.sample_rate == config.sample_rate
                && supported.channels == config.channels
                && supported.share_mode == config.share_mode
        });
        let chosen = match exact {
            Some(chosen) => *chosen,
            None if config.share_mode == ShareMode::Exclusive && config.fallback_to_shared => *self
                .configs
                .iter()
                .find(|supported| {
                    supported.sample_rate == config.sample_rate
                        && supported.channels == config.channels
                        && supported.share_mode == ShareMode::Shared
                })
                .ok_or_else(|| AudioError::UnsupportedFormat {
                    device: device.name.clone(),
                    request: config.format_request(),
                })?,
            None => {
                return Err(AudioError::UnsupportedFormat {
                    device: device.name.clone(),
                    request: config.format_request(),
                });
            }
        };
        let buffer_frames = config
            .buffer_frames
            .clamp(chosen.min_buffer_frames, chosen.max_buffer_frames);
        Ok(StreamFormat {
            device_id: device.id.clone(),
            device_name: device.name.clone(),
            sample_rate: chosen.sample_rate,
            channels: chosen.channels,
            sample_format: chosen.sample_format,
            share_mode: chosen.share_mode,
            buffer_frames,
            callback_period_frames: buffer_frames,
            stream_latency_ms: None,
            fell_back: chosen.share_mode != config.share_mode,
            fallback_reason: (chosen.share_mode != config.share_mode)
                .then(|| "requested exclusive mode, using shared".into()),
        })
    }

    /// Open a stream the caller drives manually with [`ManualStream::render_block`].
    pub fn open_manual_stream(
        &self,
        config: StreamConfig,
        callback: Box<dyn AudioCallback>,
    ) -> Result<ManualStream, AudioError> {
        if self.fail_open.load(Ordering::Relaxed) {
            return Err(AudioError::Backend("fake backend open failure".into()));
        }
        let format = self.negotiate(&config)?;
        let diagnostics = Arc::new(StreamDiagnostics::new());
        diagnostics.set_format(
            format.sample_rate,
            format.buffer_frames,
            format.callback_period_frames,
        );
        diagnostics.set_stream_latency(format.stream_latency_ms);
        diagnostics.set_state(StreamState::Running);
        let scratch = vec![0.0f32; format.buffer_frames as usize * format.channels as usize];
        Ok(ManualStream {
            format,
            diagnostics,
            callback,
            scratch,
            running: true,
        })
    }

    fn device(&self, id: &Option<String>) -> Option<DeviceInfo> {
        let devices = self.devices.lock().ok()?;
        match id {
            Some(id) => devices.iter().find(|device| &device.id == id).cloned(),
            None => devices.iter().find(|device| device.is_default).cloned(),
        }
    }
}

impl AudioBackend for FakeBackend {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn list_devices(&self) -> Result<Vec<DeviceInfo>, AudioError> {
        Ok(self
            .devices
            .lock()
            .map_err(|_| AudioError::Backend("fake device lock poisoned".into()))?
            .clone())
    }

    fn supported_configs(&self, device_id: &str) -> Result<Vec<SupportedConfig>, AudioError> {
        let devices = self
            .devices
            .lock()
            .map_err(|_| AudioError::Backend("fake device lock poisoned".into()))?;
        if devices.iter().any(|device| device.id == device_id) {
            Ok(self.configs.clone())
        } else {
            Err(AudioError::DeviceNotFound(device_id.into()))
        }
    }

    fn open_stream(
        &self,
        config: StreamConfig,
        callback: Box<dyn AudioCallback>,
    ) -> Result<RenderStream, AudioError> {
        if self.fail_open.load(Ordering::Relaxed) {
            return Err(AudioError::Backend("fake backend open failure".into()));
        }
        if self.stream_failure.load(Ordering::Relaxed) {
            return Err(AudioError::DeviceLost("simulated unplug".into()));
        }
        let format = self.negotiate(&config)?;
        let diagnostics = Arc::new(StreamDiagnostics::new());
        diagnostics.set_format(
            format.sample_rate,
            format.buffer_frames,
            format.callback_period_frames,
        );
        diagnostics.set_stream_latency(format.stream_latency_ms);
        diagnostics.set_state(StreamState::Running);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_diagnostics = Arc::clone(&diagnostics);
        let thread_format = format.clone();
        let period = self.period;
        let stream_failure = Arc::clone(&self.stream_failure);
        let expected = callback_interval(
            thread_format.callback_period_frames,
            thread_format.sample_rate,
        );
        let handle = thread::spawn(move || {
            let frames = thread_format.buffer_frames as usize;
            let mut scratch = vec![0.0f32; frames * thread_format.channels as usize];
            let mut callback = callback;
            let mut last = Instant::now();
            while !thread_stop.load(Ordering::Relaxed) {
                if stream_failure.load(Ordering::Relaxed) {
                    thread_diagnostics.mark_device_lost();
                    return;
                }
                let now = Instant::now();
                let interval = now.duration_since(last);
                last = now;
                if interval > expected * 4 && interval > period * 4 {
                    thread_diagnostics.on_late_callback(interval);
                }
                let work = Instant::now();
                callback.process(&mut scratch, frames);
                let elapsed = work.elapsed();
                thread_diagnostics.record_frames(frames);
                thread_diagnostics.on_callback(elapsed, interval.max(Duration::from_micros(1)));
                if elapsed > expected {
                    thread_diagnostics.on_xrun();
                }
                thread::sleep(period.saturating_sub(elapsed));
            }
            thread_diagnostics.set_state(StreamState::Stopped);
        });
        Ok(RenderStream::new(
            format,
            Box::new(FakeStreamControl {
                stop,
                handle: Some(handle),
                running: true,
                diagnostics,
            }),
        ))
    }
}

#[derive(Debug)]
struct FakeStreamControl {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    running: bool,
    diagnostics: Arc<StreamDiagnostics>,
}

impl StreamControl for FakeStreamControl {
    fn stop(&mut self) {
        if !self.running {
            return;
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.running = false;
    }

    fn is_running(&self) -> bool {
        self.running && self.diagnostics.state() == StreamState::Running
    }

    fn diagnostics(&self) -> &Arc<StreamDiagnostics> {
        &self.diagnostics
    }
}

/// A manually driven fake stream, for deterministic engine tests.
pub struct ManualStream {
    format: StreamFormat,
    diagnostics: Arc<StreamDiagnostics>,
    callback: Box<dyn AudioCallback>,
    scratch: Vec<f32>,
    running: bool,
}

impl ManualStream {
    /// Negotiated format.
    pub fn format(&self) -> &StreamFormat {
        &self.format
    }

    /// Shared diagnostics.
    pub fn diagnostics(&self) -> &Arc<StreamDiagnostics> {
        &self.diagnostics
    }

    /// Render one block through the callback.
    pub fn render_block(&mut self, frames: usize) {
        let frames = frames.min(self.format.buffer_frames as usize);
        let samples = frames * self.format.channels as usize;
        for sample in &mut self.scratch[..samples] {
            *sample = 0.0;
        }
        let start = Instant::now();
        self.callback.process(&mut self.scratch[..samples], frames);
        let elapsed = start.elapsed();
        self.diagnostics.record_frames(frames);
        self.diagnostics.on_callback(
            elapsed,
            callback_interval(self.format.callback_period_frames, self.format.sample_rate),
        );
    }

    /// Last rendered samples, for assertions.
    pub fn last_block(&self) -> &[f32] {
        &self.scratch
    }

    /// Stop the stream.
    pub fn stop(&mut self) {
        self.running = false;
        self.diagnostics.set_state(StreamState::Stopped);
    }

    /// True until [`ManualStream::stop`].
    pub fn is_running(&self) -> bool {
        self.running
    }
}

impl Drop for ManualStream {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    struct Counter {
        blocks: Arc<AtomicU32>,
        frames: usize,
    }

    impl AudioCallback for Counter {
        fn process(&mut self, out: &mut [f32], frames: usize) {
            self.frames += frames;
            self.blocks.fetch_add(1, Ordering::Relaxed);
            for sample in out.iter_mut().take(frames * 2) {
                *sample = 0.25;
            }
        }
    }

    #[test]
    fn fake_backend_negotiates_requested_format() {
        let backend = FakeBackend::standard();
        let format = backend.negotiate(&StreamConfig::default()).unwrap();
        assert_eq!(format.sample_rate, 48_000);
        assert_eq!(format.buffer_frames, 128);
        assert_eq!(format.sample_format, SampleFormat::F32);
        assert!(!format.fell_back);
    }

    #[test]
    fn unsupported_format_is_an_explicit_error() {
        let backend = FakeBackend::standard();
        let config = StreamConfig {
            sample_rate: 96_000,
            ..StreamConfig::default()
        };
        let error = backend.negotiate(&config).unwrap_err();
        assert!(matches!(error, AudioError::UnsupportedFormat { .. }));
    }

    #[test]
    fn exclusive_falls_back_to_shared_when_unavailable() {
        let backend = FakeBackend::standard().with_configs(vec![SupportedConfig {
            sample_rate: 48_000,
            channels: 2,
            sample_format: SampleFormat::F32,
            share_mode: ShareMode::Shared,
            min_buffer_frames: 64,
            max_buffer_frames: 2048,
        }]);
        let config = StreamConfig {
            share_mode: ShareMode::Exclusive,
            ..StreamConfig::default()
        };
        let format = backend.negotiate(&config).unwrap();
        assert_eq!(format.share_mode, ShareMode::Shared);
        assert!(format.fell_back);
    }

    #[test]
    fn buffer_size_is_clamped_to_the_device_range() {
        let backend = FakeBackend::standard();
        let config = StreamConfig {
            buffer_frames: 4,
            ..StreamConfig::default()
        };
        let format = backend.negotiate(&config).unwrap();
        assert_eq!(format.buffer_frames, 64);
    }

    #[test]
    fn manual_stream_calls_the_callback() {
        let backend = FakeBackend::standard();
        let blocks = Arc::new(AtomicU32::new(0));
        let callback = Counter {
            blocks: Arc::clone(&blocks),
            frames: 0,
        };
        let mut stream = backend
            .open_manual_stream(StreamConfig::default(), Box::new(callback))
            .unwrap();
        stream.render_block(128);
        stream.render_block(128);
        assert_eq!(blocks.load(Ordering::Relaxed), 2);
        assert_eq!(stream.diagnostics().snapshot().frames_rendered, 256);
        assert_eq!(stream.last_block()[0], 0.25);
        stream.stop();
        assert!(!stream.is_running());
    }

    #[test]
    fn threaded_fake_stream_runs_and_stops_cleanly() {
        let backend = FakeBackend::standard().with_period(Duration::from_micros(200));
        let blocks = Arc::new(AtomicU32::new(0));
        let callback = Counter {
            blocks: Arc::clone(&blocks),
            frames: 0,
        };
        let mut stream = backend
            .open_stream(StreamConfig::default(), Box::new(callback))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while blocks.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        stream.stop();
        assert!(blocks.load(Ordering::Relaxed) > 0);
        assert!(!stream.is_running());
    }

    #[test]
    fn start_stop_one_hundred_times_does_not_leak_threads() {
        let backend = FakeBackend::standard().with_period(Duration::from_micros(100));
        for _ in 0..100 {
            let callback = Counter {
                blocks: Arc::new(AtomicU32::new(0)),
                frames: 0,
            };
            let mut stream = backend
                .open_stream(StreamConfig::default(), Box::new(callback))
                .unwrap();
            stream.stop();
            assert!(!stream.is_running());
        }
    }

    #[test]
    fn fail_open_is_reported() {
        let backend = FakeBackend::standard();
        backend.set_fail_open(true);
        let error = backend
            .open_stream(
                StreamConfig::default(),
                Box::new(Counter {
                    blocks: Arc::new(AtomicU32::new(0)),
                    frames: 0,
                }),
            )
            .unwrap_err();
        assert!(matches!(error, AudioError::Backend(_)));
    }
}
