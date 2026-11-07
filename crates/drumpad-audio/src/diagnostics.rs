//! Stream diagnostics shared between the real-time audio thread and the UI.
//!
//! All counters are atomics read with `Relaxed` ordering; the audio thread
//! only performs relaxed stores. Error messages are the one exception: they
//! are written behind a mutex, but only on the failure path, never while the
//! stream is healthy.

use crate::LatencyEstimate;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// High-level stream state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// No stream is open.
    Stopped,
    /// The stream thread is initializing.
    Starting,
    /// The stream is rendering.
    Running,
    /// The device disappeared.
    DeviceLost,
    /// The stream failed for another reason.
    Error,
}

/// Numeric error kind published from the real-time thread without allocating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorCode {
    None,
    DeviceLost,
    Backend,
}

impl ErrorCode {
    fn as_u8(self) -> u8 {
        match self {
            ErrorCode::None => 0,
            ErrorCode::DeviceLost => 1,
            ErrorCode::Backend => 2,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => ErrorCode::DeviceLost,
            2 => ErrorCode::Backend,
            _ => ErrorCode::None,
        }
    }

    fn message(self) -> Option<&'static str> {
        match self {
            ErrorCode::None => None,
            ErrorCode::DeviceLost => Some("audio device was lost"),
            ErrorCode::Backend => Some("audio backend error"),
        }
    }
}

impl StreamState {
    fn as_u8(self) -> u8 {
        match self {
            StreamState::Stopped => 0,
            StreamState::Starting => 1,
            StreamState::Running => 2,
            StreamState::DeviceLost => 3,
            StreamState::Error => 4,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => StreamState::Starting,
            2 => StreamState::Running,
            3 => StreamState::DeviceLost,
            4 => StreamState::Error,
            _ => StreamState::Stopped,
        }
    }
}

/// A point-in-time copy of the stream counters.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamSnapshot {
    /// Stream state.
    pub state: StreamState,
    /// Last error message, if any.
    pub error: Option<String>,
    /// Render callbacks served.
    pub callbacks: u64,
    /// Frames rendered to the device.
    pub frames_rendered: u64,
    /// Late callbacks and detected buffer underruns.
    pub xruns: u64,
    /// Callbacks whose interval exceeded 1.5 periods.
    pub late_callbacks: u64,
    /// Event waits that timed out.
    pub timeouts: u64,
    /// Exponentially averaged callback load in parts per million.
    pub callback_load_ppm: u32,
    /// Highest callback load seen, in parts per million.
    pub callback_load_max_ppm: u32,
    /// Last observed callback interval in microseconds.
    pub last_interval_us: u32,
    /// Configured sample rate.
    pub sample_rate: u32,
    /// Device buffer size in frames.
    pub buffer_frames: u32,
    /// One callback period in frames.
    pub callback_period_frames: u32,
    /// Driver-reported stream latency in milliseconds, when available.
    pub stream_latency_ms: Option<f32>,
}

impl StreamSnapshot {
    /// Callback load as a fraction of real time.
    pub fn callback_load(&self) -> f32 {
        self.callback_load_ppm as f32 / 1_000_000.0
    }

    /// Latency estimate derived from the configured stream.
    pub fn latency(&self) -> LatencyEstimate {
        LatencyEstimate::from_frames(
            self.sample_rate,
            self.buffer_frames,
            self.callback_period_frames,
            self.stream_latency_ms,
        )
    }
}

/// Counters written by the stream thread and read by the UI.
#[derive(Debug)]
pub struct StreamDiagnostics {
    state: AtomicU8,
    callbacks: AtomicU64,
    frames_rendered: AtomicU64,
    xruns: AtomicU64,
    late_callbacks: AtomicU64,
    timeouts: AtomicU64,
    callback_load_ppm: AtomicU32,
    callback_load_max_ppm: AtomicU32,
    last_interval_us: AtomicU32,
    sample_rate: AtomicU32,
    buffer_frames: AtomicU32,
    callback_period_frames: AtomicU32,
    stream_latency_ms: AtomicU32,
    error_code: AtomicU8,
    error: Mutex<Option<String>>,
}

impl Default for StreamDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamDiagnostics {
    /// Create a stopped, zeroed diagnostics block.
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(StreamState::Stopped.as_u8()),
            callbacks: AtomicU64::new(0),
            frames_rendered: AtomicU64::new(0),
            xruns: AtomicU64::new(0),
            late_callbacks: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
            callback_load_ppm: AtomicU32::new(0),
            callback_load_max_ppm: AtomicU32::new(0),
            last_interval_us: AtomicU32::new(0),
            sample_rate: AtomicU32::new(0),
            buffer_frames: AtomicU32::new(0),
            callback_period_frames: AtomicU32::new(0),
            stream_latency_ms: AtomicU32::new(f32::NAN.to_bits()),
            error_code: AtomicU8::new(ErrorCode::None.as_u8()),
            error: Mutex::new(None),
        }
    }

    /// Record the negotiated stream format.
    pub fn set_format(&self, sample_rate: u32, buffer_frames: u32, callback_period_frames: u32) {
        self.sample_rate.store(sample_rate, Ordering::Relaxed);
        self.buffer_frames.store(buffer_frames, Ordering::Relaxed);
        self.callback_period_frames
            .store(callback_period_frames, Ordering::Relaxed);
    }

    /// Record the driver-reported stream latency (`None` when not reported).
    pub fn set_stream_latency(&self, latency_ms: Option<f32>) {
        let bits = latency_ms
            .filter(|value| value.is_finite())
            .map_or(f32::NAN.to_bits(), f32::to_bits);
        self.stream_latency_ms.store(bits, Ordering::Relaxed);
    }

    /// Latest reported stream latency.
    pub fn stream_latency_ms(&self) -> Option<f32> {
        let value = f32::from_bits(self.stream_latency_ms.load(Ordering::Relaxed));
        value.is_finite().then_some(value)
    }

    /// Set the high-level state.
    pub fn set_state(&self, state: StreamState) {
        self.state.store(state.as_u8(), Ordering::Relaxed);
    }

    /// Current state.
    pub fn state(&self) -> StreamState {
        StreamState::from_u8(self.state.load(Ordering::Relaxed))
    }

    /// Store a detailed error message and move to the error state. This
    /// allocates, so it is for non-real-time callers (startup/open paths) only.
    pub fn set_error(&self, message: impl Into<String>) {
        self.error_code
            .store(ErrorCode::Backend.as_u8(), Ordering::Relaxed);
        if let Ok(mut slot) = self.error.lock() {
            *slot = Some(message.into());
        }
        self.set_state(StreamState::Error);
    }

    /// Store a detailed device-loss message. Non-real-time callers only.
    pub fn set_device_lost(&self, message: impl Into<String>) {
        self.error_code
            .store(ErrorCode::DeviceLost.as_u8(), Ordering::Relaxed);
        if let Ok(mut slot) = self.error.lock() {
            *slot = Some(message.into());
        }
        self.set_state(StreamState::DeviceLost);
    }

    /// Mark a backend error from the real-time thread. No allocation, no lock.
    pub fn mark_error(&self) {
        self.error_code
            .store(ErrorCode::Backend.as_u8(), Ordering::Relaxed);
        self.set_state(StreamState::Error);
    }

    /// Mark a device loss from the real-time thread. No allocation, no lock.
    pub fn mark_device_lost(&self) {
        self.error_code
            .store(ErrorCode::DeviceLost.as_u8(), Ordering::Relaxed);
        self.set_state(StreamState::DeviceLost);
    }

    /// Last error, cloned out of the mutex or derived from the atomic code.
    pub fn error(&self) -> Option<String> {
        let detailed = self.error.lock().ok().and_then(|slot| slot.clone());
        detailed.or_else(|| {
            ErrorCode::from_u8(self.error_code.load(Ordering::Relaxed))
                .message()
                .map(str::to_string)
        })
    }

    /// Clear the error message and code.
    pub fn clear_error(&self) {
        self.error_code
            .store(ErrorCode::None.as_u8(), Ordering::Relaxed);
        if let Ok(mut slot) = self.error.lock() {
            *slot = None;
        }
    }

    /// Record one successful callback.
    pub fn on_callback(&self, elapsed: Duration, interval: Duration) {
        let index = self.callbacks.fetch_add(1, Ordering::Relaxed);
        let interval_us = interval.as_micros().min(u32::MAX as u128) as u32;
        self.last_interval_us.store(interval_us, Ordering::Relaxed);
        // The first callback includes stream start-up work; it says nothing
        // about steady-state load, so only count it.
        if index == 0 {
            return;
        }
        let load = if interval_us > 0 {
            (elapsed.as_micros() as f64 / interval_us as f64 * 1_000_000.0).min(u32::MAX as f64)
                as u32
        } else {
            0
        };
        let previous = self.callback_load_ppm.load(Ordering::Relaxed);
        let smoothed = if previous == 0 {
            load
        } else {
            ((previous as u64 * 7 + load as u64) / 8) as u32
        };
        self.callback_load_ppm.store(smoothed, Ordering::Relaxed);
        if load > self.callback_load_max_ppm.load(Ordering::Relaxed) {
            self.callback_load_max_ppm.store(load, Ordering::Relaxed);
        }
    }

    /// Record a callback that arrived later than 1.5 periods. This is an xrun
    /// for a render stream: the device almost certainly played silence.
    pub fn on_late_callback(&self, interval: Duration) {
        self.late_callbacks.fetch_add(1, Ordering::Relaxed);
        self.xruns.fetch_add(1, Ordering::Relaxed);
        let interval_us = interval.as_micros().min(u32::MAX as u128) as u32;
        self.last_interval_us.store(interval_us, Ordering::Relaxed);
    }

    /// Record a buffer underrun detected while writing.
    pub fn on_xrun(&self) {
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an event wait timeout.
    pub fn on_timeout(&self) {
        self.timeouts.fetch_add(1, Ordering::Relaxed);
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }

    /// Record rendered frames.
    pub fn record_frames(&self, frames: usize) {
        self.frames_rendered
            .fetch_add(frames as u64, Ordering::Relaxed);
    }

    /// Snapshot all counters for the UI.
    pub fn snapshot(&self) -> StreamSnapshot {
        StreamSnapshot {
            state: self.state(),
            error: self.error(),
            callbacks: self.callbacks.load(Ordering::Relaxed),
            frames_rendered: self.frames_rendered.load(Ordering::Relaxed),
            xruns: self.xruns.load(Ordering::Relaxed),
            late_callbacks: self.late_callbacks.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            callback_load_ppm: self.callback_load_ppm.load(Ordering::Relaxed),
            callback_load_max_ppm: self.callback_load_max_ppm.load(Ordering::Relaxed),
            last_interval_us: self.last_interval_us.load(Ordering::Relaxed),
            sample_rate: self.sample_rate.load(Ordering::Relaxed),
            buffer_frames: self.buffer_frames.load(Ordering::Relaxed),
            callback_period_frames: self.callback_period_frames.load(Ordering::Relaxed),
            stream_latency_ms: self.stream_latency_ms(),
        }
    }

    /// Reset the counters without touching the format.
    pub fn reset_counters(&self) {
        self.callbacks.store(0, Ordering::Relaxed);
        self.frames_rendered.store(0, Ordering::Relaxed);
        self.xruns.store(0, Ordering::Relaxed);
        self.late_callbacks.store(0, Ordering::Relaxed);
        self.timeouts.store(0, Ordering::Relaxed);
        self.callback_load_ppm.store(0, Ordering::Relaxed);
        self.callback_load_max_ppm.store(0, Ordering::Relaxed);
        self.last_interval_us.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{SampleFormat, ShareMode, StreamFormat};

    #[test]
    fn callback_load_and_peak_are_tracked() {
        let diagnostics = StreamDiagnostics::new();
        diagnostics.set_format(48_000, 128, 128);
        // The first callback is a warm-up and only counts.
        diagnostics.on_callback(Duration::from_micros(100), Duration::from_micros(2_667));
        diagnostics.on_callback(Duration::from_micros(100), Duration::from_micros(2_667));
        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.callbacks, 2);
        assert!(snapshot.callback_load_ppm > 30_000);
        assert!(snapshot.callback_load_ppm < 40_000);
        assert!(snapshot.callback_load_max_ppm > 30_000);
        diagnostics.on_callback(Duration::from_micros(2_000), Duration::from_micros(2_667));
        let snapshot = diagnostics.snapshot();
        assert!(snapshot.callback_load_max_ppm > snapshot.callback_load_ppm);
    }

    #[test]
    fn late_callbacks_and_timeouts_count_as_xruns() {
        let diagnostics = StreamDiagnostics::new();
        diagnostics.on_late_callback(Duration::from_millis(20));
        diagnostics.on_timeout();
        diagnostics.on_xrun();
        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.late_callbacks, 1);
        assert_eq!(snapshot.timeouts, 1);
        assert_eq!(snapshot.xruns, 3);
        assert_eq!(snapshot.last_interval_us, 20_000);
    }

    #[test]
    fn error_state_is_visible() {
        let diagnostics = StreamDiagnostics::new();
        diagnostics.set_error("boom");
        assert_eq!(diagnostics.state(), StreamState::Error);
        assert_eq!(diagnostics.snapshot().error.as_deref(), Some("boom"));
        diagnostics.clear_error();
        assert_eq!(diagnostics.snapshot().error, None);
    }

    #[test]
    fn latency_estimate_matches_frames() {
        let estimate = LatencyEstimate::from_frames(48_000, 256, 128, Some(1.5));
        assert!((estimate.buffer_ms - 5.333).abs() < 0.01);
        assert!((estimate.callback_period_ms - 2.667).abs() < 0.01);
        assert!((estimate.estimated_end_to_end_ms - 5.333).abs() < 0.01);
    }

    #[test]
    fn stream_format_latency_uses_max_not_sum() {
        let format = StreamFormat {
            device_id: "id".into(),
            device_name: "Speakers".into(),
            sample_rate: 48_000,
            channels: 2,
            sample_format: SampleFormat::I16,
            share_mode: ShareMode::Exclusive,
            buffer_frames: 256,
            callback_period_frames: 256,
            stream_latency_ms: Some(5.333),
            fell_back: false,
            fallback_reason: None,
        };
        assert!((format.estimated_latency_ms() - 5.333).abs() < 0.01);
    }

    #[test]
    fn diagnostics_publish_stream_latency() {
        let diagnostics = StreamDiagnostics::new();
        diagnostics.set_format(48_000, 256, 256);
        diagnostics.set_stream_latency(Some(5.333));
        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.stream_latency_ms, Some(5.333));
        assert!((snapshot.latency().estimated_end_to_end_ms - 5.333).abs() < 0.01);
        diagnostics.set_stream_latency(None);
        assert_eq!(diagnostics.snapshot().stream_latency_ms, None);
    }
}
