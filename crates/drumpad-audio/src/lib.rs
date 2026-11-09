//! Windows audio devices, stream lifecycle, and the WASAPI backend.
//!
//! The crate is split so the UI can depend on [`backend::AudioBackend`] and
//! [`devices::DeviceInfo`] without ever seeing a WASAPI type. Tests use the
//! deterministic fake backend in [`backend::FakeBackend`].

pub mod backend;
pub mod devices;
pub mod diagnostics;
pub mod priority;
pub mod runtime;
pub mod settings;
pub mod stream;
pub mod system;

pub use backend::{AudioBackend, AudioCallback, FakeBackend};
pub use devices::{DeviceInfo, DeviceProvider, DeviceState};
pub use diagnostics::{StreamDiagnostics, StreamSnapshot, StreamState};
pub use priority::boost_audio_thread;
pub use runtime::MvpAudioRuntime;
pub use settings::{AudioSettings, SettingsLoad};
pub use stream::{RenderStream, SampleFormat, ShareMode, StreamConfig, StreamFormat};
pub use system::{AudioSystem, AudioSystemEvent, AudioSystemState, CallbackFactory};

use std::error::Error;
use std::fmt;

/// Errors produced by the audio layer. Payloads are strings so the type can
/// cross thread and store boundaries without holding COM objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    /// No render device is available at all.
    NoDevice,
    /// The requested device id or name was not found.
    DeviceNotFound(String),
    /// The device exists but cannot run the requested format/mode.
    UnsupportedFormat {
        /// Device name or id.
        device: String,
        /// Human-readable format request.
        request: String,
    },
    /// The device disappeared while a stream was running.
    DeviceLost(String),
    /// WASAPI or the fake backend failed for another reason.
    Backend(String),
    /// Local persistence or I/O failed.
    Io(String),
    /// The demo kit or MVP runtime could not be prepared.
    Runtime(String),
    /// The stream is already stopped.
    StreamClosed,
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoDevice => write!(f, "no audio render device is available"),
            AudioError::DeviceNotFound(id) => write!(f, "audio device not found: {id}"),
            AudioError::UnsupportedFormat { device, request } => {
                write!(f, "device {device} does not support {request}")
            }
            AudioError::DeviceLost(id) => write!(f, "audio device lost: {id}"),
            AudioError::Backend(message) => write!(f, "audio backend error: {message}"),
            AudioError::Io(message) => write!(f, "audio settings I/O error: {message}"),
            AudioError::Runtime(message) => write!(f, "audio runtime error: {message}"),
            AudioError::StreamClosed => write!(f, "audio stream is already closed"),
        }
    }
}

impl Error for AudioError {}

impl From<std::io::Error> for AudioError {
    fn from(value: std::io::Error) -> Self {
        AudioError::Io(value.to_string())
    }
}

impl From<serde_json::Error> for AudioError {
    fn from(value: serde_json::Error) -> Self {
        AudioError::Io(value.to_string())
    }
}

#[cfg(windows)]
impl From<wasapi::WasapiError> for AudioError {
    fn from(value: wasapi::WasapiError) -> Self {
        AudioError::Backend(value.to_string())
    }
}

/// Total latency estimate for a stream, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LatencyEstimate {
    /// One callback period, `buffer_frames / sample_rate`.
    pub callback_period_ms: f32,
    /// The device buffer, `buffer_frames / sample_rate`.
    pub buffer_ms: f32,
    /// The driver-reported stream latency, when available.
    pub stream_latency_ms: Option<f32>,
    /// Software end-to-end estimate before DAC/analog stages.
    pub estimated_end_to_end_ms: f32,
}

impl LatencyEstimate {
    /// Build an estimate from frame counts and an optional driver report.
    pub fn from_frames(
        sample_rate: u32,
        buffer_frames: u32,
        callback_period_frames: u32,
        stream_latency_ms: Option<f32>,
    ) -> Self {
        let rate = sample_rate.max(1) as f32;
        let buffer_ms = buffer_frames as f32 / rate * 1000.0;
        let callback_period_ms = callback_period_frames as f32 / rate * 1000.0;
        // `GetStreamLatency` already includes the queued device buffer where
        // the driver reports it, so take the larger value rather than adding
        // the two. This excludes converter, DAC, and analog stages.
        let estimated_end_to_end_ms = buffer_ms
            .max(callback_period_ms)
            .max(stream_latency_ms.unwrap_or(0.0));
        Self {
            callback_period_ms,
            buffer_ms,
            stream_latency_ms,
            estimated_end_to_end_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_identifies_the_failure() {
        let cases = [
            (AudioError::NoDevice, "no audio render device"),
            (AudioError::DeviceNotFound("dev".into()), "dev"),
            (
                AudioError::UnsupportedFormat {
                    device: "Speakers".into(),
                    request: "48000 Hz".into(),
                },
                "Speakers",
            ),
            (AudioError::DeviceLost("gone".into()), "gone"),
            (AudioError::Backend("wasapi".into()), "wasapi"),
            (AudioError::Io("disk".into()), "disk"),
            (AudioError::Runtime("kit".into()), "kit"),
            (AudioError::StreamClosed, "closed"),
        ];
        for (error, needle) in cases {
            let text = error.to_string();
            assert!(
                text.to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase()),
                "{text:?} did not contain {needle:?}"
            );
        }
    }

    #[test]
    fn io_and_json_errors_map_to_the_io_variant() {
        let io = AudioError::from(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing settings",
        ));
        assert!(matches!(io, AudioError::Io(message) if message.contains("missing settings")));

        let json = serde_json::from_str::<AudioSettings>("{ not json").unwrap_err();
        assert!(matches!(AudioError::from(json), AudioError::Io(_)));
    }

    #[test]
    fn latency_estimate_takes_the_largest_contribution() {
        let buffer_bound = LatencyEstimate::from_frames(48_000, 256, 128, None);
        assert!((buffer_bound.buffer_ms - 5.333).abs() < 0.01);
        assert!((buffer_bound.callback_period_ms - 2.667).abs() < 0.01);
        assert!((buffer_bound.estimated_end_to_end_ms - 5.333).abs() < 0.01);

        let driver_bound = LatencyEstimate::from_frames(48_000, 128, 128, Some(12.0));
        assert_eq!(driver_bound.estimated_end_to_end_ms, 12.0);

        let zero_rate = LatencyEstimate::from_frames(0, 0, 0, None);
        assert!(zero_rate.buffer_ms.is_finite());
        assert_eq!(zero_rate.buffer_ms, 0.0);
    }
}
