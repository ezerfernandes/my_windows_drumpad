//! Small MVP runtime that connects the demo kit to the default output device.
//!
//! The app needs one call to make sound: [`MvpAudioRuntime::start`] builds the
//! embedded demo kit into a [`SampleBank`], creates a 48 kHz [`Engine`], and
//! opens an [`AudioSystem`] whose callback owns that engine. The UI then only
//! needs [`MvpAudioRuntime::trigger`], [`MvpAudioRuntime::release`] and
//! [`MvpAudioRuntime::panic`].
//!
//! This is deliberately a vertical-slice facade, not the general engine API.
//! Kit editing, per-pad sample replacement, exclusive-mode selection, and
//! device-loss UI are follow-ups.

use crate::backend::{AudioBackend, AudioCallback, WindowsBackend};
use crate::system::{AudioSystem, AudioSystemEvent, AudioSystemState};
use crate::{AudioError, StreamFormat, StreamSnapshot};
use drumpad_engine::mixer::{
    Engine, EngineConfig, EngineHandle, EngineSnapshot, PAD_COUNT, PadSettings,
};
use drumpad_engine::queue::Command;
use drumpad_engine::voice::{LoopPoints, PlayMode, Playback, SampleBank, SampleBuffer};
use drumpad_input::trigger::TriggerSource;
use drumpad_model::ids::{PadId, Velocity};
use drumpad_model::kit::{DEMO_SAMPLES, PlaybackMode, demo_kit};
use drumpad_samples::decode::decode_file;
use drumpad_samples::resample::prepare_for_device;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// MVP sample rate. The stream request is pinned to this rate so the engine
/// increment and the device rate always agree.
pub const MVP_SAMPLE_RATE: u32 = 48_000;

/// Block size used for the startup pre-roll and the default stream request.
pub const MVP_BLOCK_FRAMES: usize = 128;

/// The app-facing audio facade for the MVP vertical slice.
pub struct MvpAudioRuntime {
    system: AudioSystem,
    handle: Arc<Mutex<Option<EngineHandle>>>,
    pad_names: Vec<Option<String>>,
    sample_rate: u32,
}

impl MvpAudioRuntime {
    /// Build the demo kit and open the default Windows render device in shared
    /// mode. Returns an error if the demo kit cannot be decoded or the device
    /// cannot open.
    pub fn start() -> Result<Self, AudioError> {
        Self::start_with_backend(
            Box::new(WindowsBackend),
            None,
            EngineConfig {
                sample_rate: MVP_SAMPLE_RATE,
                voice_limit: 64,
                ..EngineConfig::default()
            },
        )
    }

    /// Build the demo kit against a specific backend. `settings_path` is
    /// optional; when absent the MVP uses 48 kHz shared mode defaults.
    ///
    /// This is public so deterministic tests can use [`crate::FakeBackend`].
    pub fn start_with_backend(
        backend: Box<dyn AudioBackend>,
        settings_path: Option<PathBuf>,
        mut config: EngineConfig,
    ) -> Result<Self, AudioError> {
        config.sample_rate = config.sample_rate.max(8_000);
        let sample_rate = config.sample_rate;
        let (bank, pads, pad_names) = build_demo_bank(sample_rate)?;

        let handle_slot: Arc<Mutex<Option<EngineHandle>>> = Arc::new(Mutex::new(None));
        let factory_slot = Arc::clone(&handle_slot);
        let factory_bank = Arc::clone(&bank);
        let factory_config = config.clone();
        let factory = Box::new(move || {
            let (mut engine, mut handle) = Engine::new(factory_config.clone());
            let _ = handle.send(Command::SwapSampleBank(Arc::clone(&factory_bank)));
            for (index, settings) in pads.iter().enumerate() {
                let pad = PadId::from_raw_unchecked(index as u8);
                if let Some(slot) = settings.sample_slot {
                    let _ = handle.send(Command::SetPadSample { pad, slot });
                }
                let _ = handle.send(Command::SetPadPlayback {
                    pad,
                    playback: settings.playback,
                });
                let _ = handle.send(Command::SetPadGain {
                    pad,
                    gain: settings.gain,
                });
                let _ = handle.send(Command::SetPadPan {
                    pad,
                    pan: settings.pan,
                });
                let _ = handle.send(Command::SetPadTune {
                    pad,
                    cents: settings.tune_cents,
                });
                let _ = handle.send(Command::SetPadChokeGroup {
                    pad,
                    group: settings.choke_group,
                });
            }
            // Apply the bank + pad commands before the stream starts; the
            // first real callback then has a fully configured engine.
            let mut silence = vec![0.0f32; MVP_BLOCK_FRAMES * 2];
            engine.process(&mut silence, MVP_BLOCK_FRAMES);
            if let Ok(mut slot) = factory_slot.lock() {
                *slot = Some(handle);
            }
            Box::new(EngineCallback { engine }) as Box<dyn AudioCallback>
        });

        let has_settings = settings_path.is_some();
        let mut system = AudioSystem::new(backend, settings_path, factory);
        if has_settings {
            let _ = system.load_settings();
        }
        // Pin the stream request to the same rate the bank was resampled to.
        let mut settings = system.settings().clone();
        settings.sample_rate = sample_rate;
        settings.buffer_frames = MVP_BLOCK_FRAMES as u32;
        settings.validate();
        system.set_settings(settings);
        if let Err(error) = system.start() {
            if let Ok(mut slot) = handle_slot.lock() {
                *slot = None;
            }
            return Err(error);
        }
        let negotiated_rate = system.format().map(|format| format.sample_rate);
        if let Some(rate) = negotiated_rate
            && rate != sample_rate
        {
            system.stop();
            if let Ok(mut slot) = handle_slot.lock() {
                *slot = None;
            }
            return Err(AudioError::Runtime(format!(
                "device negotiated {rate} Hz but the engine was built for {sample_rate} Hz"
            )));
        }

        Ok(Self {
            system,
            handle: handle_slot,
            pad_names,
            sample_rate,
        })
    }

    /// Trigger a pad from the UI. `pad_index` is clamped to `0..=15`;
    /// `velocity` is clamped to `1..=127`.
    pub fn trigger(&self, pad_index: u8, velocity: u8) {
        if let Ok(slot) = self.handle.lock()
            && let Some(handle) = slot.as_ref()
        {
            let _ = handle.note_on(
                PadId::clamped(pad_index),
                Velocity::clamped(velocity),
                TriggerSource::UiPreview,
            );
        }
    }

    /// Release a gated pad from the UI.
    pub fn release(&self, pad_index: u8) {
        if let Ok(slot) = self.handle.lock()
            && let Some(handle) = slot.as_ref()
        {
            let _ = handle.note_off(PadId::clamped(pad_index), TriggerSource::UiPreview);
        }
    }

    /// Stop every voice with a short fade.
    pub fn panic(&self) {
        if let Ok(mut slot) = self.handle.lock()
            && let Some(handle) = slot.as_mut()
        {
            let _ = handle.panic();
        }
    }

    /// True while the output stream is rendering.
    pub fn is_running(&self) -> bool {
        self.system.state().is_running()
    }

    /// Negotiated stream format, once open.
    pub fn format(&self) -> Option<&StreamFormat> {
        self.system.format()
    }

    /// Current high-level audio state (for a status label or error dialog).
    pub fn state(&self) -> &AudioSystemState {
        self.system.state()
    }

    /// Latest stream counters.
    pub fn snapshot(&self) -> Option<StreamSnapshot> {
        self.system.snapshot()
    }

    /// Latest engine meters/voice count, if a stream is open.
    pub fn engine_snapshot(&self) -> Option<EngineSnapshot> {
        let slot = self.handle.lock().ok()?;
        let handle = slot.as_ref()?;
        let meter = handle.meter();
        Some(EngineSnapshot {
            meter: meter.values(),
            active_voices: meter.active_voices() as usize,
            voice_limit: meter.voice_limit() as usize,
            playhead: handle.timestamp_now(),
        })
    }

    /// Number of voices currently producing samples.
    pub fn active_voices(&self) -> u32 {
        self.engine_snapshot()
            .map_or(0, |snapshot| snapshot.active_voices as u32)
    }

    /// Pad display names from the demo kit, indexed 0..16.
    pub fn pad_names(&self) -> &[Option<String>] {
        &self.pad_names
    }

    /// Sample rate the engine and stream were built for.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Poll the audio system. Call this from the UI at ~10 Hz.
    pub fn tick(&mut self) -> Option<AudioSystemEvent> {
        self.system.tick()
    }

    /// Drain pending audio-system events.
    pub fn drain_events(&mut self) -> Vec<AudioSystemEvent> {
        self.system.drain_events()
    }

    /// Stop the stream and drop the current engine handle.
    pub fn stop(&mut self) {
        self.system.stop();
        if let Ok(mut slot) = self.handle.lock() {
            *slot = None;
        }
    }
}

impl Drop for MvpAudioRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

impl std::fmt::Debug for MvpAudioRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MvpAudioRuntime")
            .field("sample_rate", &self.sample_rate)
            .field("state", &self.system.state())
            .field("format", &self.system.format())
            .finish()
    }
}

struct EngineCallback {
    engine: Engine,
}

impl AudioCallback for EngineCallback {
    fn process(&mut self, out: &mut [f32], frames: usize) {
        self.engine.process(out, frames);
    }
}

/// A decoded demo bank plus the per-pad runtime settings and display names.
type DemoBank = (
    Arc<SampleBank>,
    [PadSettings; PAD_COUNT],
    Vec<Option<String>>,
);

/// Decode the embedded demo WAVs, resample them to `device_rate`, and map them
/// to pad runtime settings. No audio-thread work happens here.
fn build_demo_bank(device_rate: u32) -> Result<DemoBank, AudioError> {
    let kit = demo_kit()
        .map_err(|error| AudioError::Runtime(format!("embedded demo kit is invalid: {error}")))?;
    let temp = demo_temp_dir();
    std::fs::create_dir_all(&temp).map_err(|error| {
        AudioError::Runtime(format!(
            "could not create demo sample folder {}: {error}",
            temp.display()
        ))
    })?;

    let mut decoded: HashMap<String, SampleBuffer> = HashMap::with_capacity(DEMO_SAMPLES.len());
    for (relative, bytes) in DEMO_SAMPLES {
        let path = temp.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                AudioError::Runtime(format!("could not create {}: {error}", parent.display()))
            })?;
        }
        std::fs::write(&path, bytes).map_err(|error| {
            AudioError::Runtime(format!("could not write {}: {error}", path.display()))
        })?;
        let decoded_audio = decode_file(&path).map_err(|error| {
            AudioError::Runtime(format!("could not decode demo sample {relative}: {error}"))
        })?;
        let prepared =
            prepare_for_device(&decoded_audio.audio, device_rate, false).map_err(|error| {
                AudioError::Runtime(format!(
                    "could not resample demo sample {relative} to {device_rate} Hz: {error}"
                ))
            })?;
        let buffer = prepared.into_sample_buffer().map_err(|error| {
            AudioError::Runtime(format!(
                "demo sample {relative} is not a valid playback buffer: {error}"
            ))
        })?;
        decoded.insert((*relative).to_owned(), buffer);
    }
    let _ = std::fs::remove_dir_all(&temp);

    let mut bank = SampleBank::new();
    let mut pads = [PadSettings::default(); PAD_COUNT];
    let mut names: Vec<Option<String>> = vec![None; PAD_COUNT];
    for pad in &kit.pads {
        let index = pad.id.index();
        if index >= PAD_COUNT {
            continue;
        }
        names[index] = pad.name.clone();
        let Some(layer) = pad.layers.iter().find(|layer| layer.sample.is_some()) else {
            continue;
        };
        let Some(sample_ref) = layer.sample.as_ref() else {
            continue;
        };
        let Some(sample) = decoded.get(sample_ref.path.as_str()) else {
            continue;
        };
        let slot = bank.push(Arc::new(sample.clone()));
        let gain_db = pad.gain_db + layer.gain_db;
        let loop_points = layer
            .loop_points
            .map(|points| LoopPoints {
                start: points.start_frame.min(u64::from(u32::MAX)) as u32,
                end: points.end_frame.min(u64::from(u32::MAX)) as u32,
                crossfade: 0,
            })
            .unwrap_or_default();
        pads[index] = PadSettings {
            sample_slot: Some(slot),
            gain: 10f32.powf(gain_db / 20.0).clamp(0.0, 2.0),
            pan: pad.pan.clamp(-1.0, 1.0),
            tune_cents: (pad.tune_semitones * 100.0).round() as i32
                + layer.tune_cents.round() as i32,
            choke_group: pad.choke_group.map(|group| group.get()).unwrap_or(0),
            playback: Playback {
                mode: match pad.playback {
                    PlaybackMode::OneShot => PlayMode::OneShot,
                    PlaybackMode::Gate => PlayMode::Gate,
                    PlaybackMode::Loop => PlayMode::Loop,
                },
                loop_points,
            },
        };
    }
    Ok((Arc::new(bank), pads, names))
}

fn demo_temp_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos();
    std::env::temp_dir().join(format!(
        "drumpad-demo-{}-{stamp}-{sequence}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::FakeBackend;
    use std::time::Instant;

    #[test]
    fn demo_bank_maps_all_sixteen_pads() {
        let (bank, pads, names) = build_demo_bank(MVP_SAMPLE_RATE).unwrap();
        assert_eq!(bank.len(), 16);
        assert_eq!(pads.len(), 16);
        assert_eq!(names[0].as_deref(), Some("Kick"));
        assert!(pads.iter().all(|pad| pad.sample_slot.is_some()));
        assert!(pads.iter().any(|pad| pad.choke_group == 1));
        assert!(pads.iter().all(|pad| pad.gain.is_finite()));
    }

    #[test]
    fn runtime_starts_with_fake_backend_and_plays() {
        let backend = FakeBackend::standard().with_period(Duration::from_millis(2));
        let mut runtime = MvpAudioRuntime::start_with_backend(
            Box::new(backend),
            None,
            EngineConfig {
                sample_rate: MVP_SAMPLE_RATE,
                voice_limit: 64,
                ..EngineConfig::default()
            },
        )
        .expect("runtime");
        assert!(runtime.is_running());
        assert_eq!(runtime.sample_rate(), MVP_SAMPLE_RATE);

        runtime.trigger(0, 100);
        let deadline = Instant::now() + Duration::from_secs(2);
        while runtime.active_voices() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(runtime.active_voices() > 0, "trigger did not start a voice");
        assert!(runtime.snapshot().is_some_and(|s| s.callbacks > 0));

        runtime.panic();
        let deadline = Instant::now() + Duration::from_secs(2);
        while runtime.active_voices() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(runtime.active_voices(), 0, "panic left a stuck voice");
        runtime.stop();
        assert!(!runtime.is_running());
    }

    #[test]
    fn out_of_range_pad_and_velocity_are_clamped() {
        let backend = FakeBackend::standard().with_period(Duration::from_millis(2));
        let runtime = MvpAudioRuntime::start_with_backend(
            Box::new(backend),
            None,
            EngineConfig {
                sample_rate: MVP_SAMPLE_RATE,
                ..EngineConfig::default()
            },
        )
        .expect("runtime");
        runtime.trigger(u8::MAX, u8::MAX);
        runtime.release(u8::MAX);
        runtime.panic();
    }
}
