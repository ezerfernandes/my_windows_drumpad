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
use drumpad_model::kit::{DEMO_SAMPLES, Kit, PlaybackMode, demo_kit};
use drumpad_samples::decode::decode_file;
use drumpad_samples::resample::prepare_for_device;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// MVP sample rate. The stream request is pinned to this rate so the engine
/// increment and the device rate always agree.
pub const MVP_SAMPLE_RATE: u32 = 48_000;

/// Block size used for the startup pre-roll and the default stream request.
pub const MVP_BLOCK_FRAMES: usize = 128;

/// The bank and pad settings that the next callback factory invocation should
/// install. `load_kit` updates this snapshot so a device-loss reconnect uses the
/// most recently loaded kit instead of the startup demo bank.
#[derive(Clone)]
struct RuntimeKitState {
    bank: Arc<SampleBank>,
    pads: [PadSettings; PAD_COUNT],
}

/// The app-facing audio facade for the MVP vertical slice.
pub struct MvpAudioRuntime {
    system: AudioSystem,
    handle: Arc<Mutex<Option<EngineHandle>>>,
    kit_state: Arc<Mutex<RuntimeKitState>>,
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
        let kit_state = Arc::new(Mutex::new(RuntimeKitState { bank, pads }));
        let factory_slot = Arc::clone(&handle_slot);
        let factory_kit = Arc::clone(&kit_state);
        let factory_config = config.clone();
        let factory = Box::new(move || {
            let (bank, pads) = {
                let state = match factory_kit.lock() {
                    Ok(state) => state,
                    Err(poisoned) => poisoned.into_inner(),
                };
                (Arc::clone(&state.bank), state.pads)
            };
            let (mut engine, mut handle) = Engine::new(factory_config.clone());
            let _ = handle.send(Command::SwapSampleBank(bank));
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
            kit_state,
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

    /// Replace the live sample bank with `kit`, resolving samples through
    /// `kit_dir`.
    ///
    /// The kit is decoded and resampled on the calling (UI) thread, then the
    /// bank and pad settings are sent through the running engine handle. Active
    /// voices are faded first (a bounded wait) so the swap is click-free; the
    /// engine installs the new bank at the start of the next audio block.
    /// Pad names are updated on success.
    ///
    /// Returns [`AudioError::Runtime`] for a missing sample, an empty bank, a
    /// stopped stream, or a full command queue.
    pub fn load_kit(&mut self, kit: &Kit, kit_dir: &Path) -> Result<(), AudioError> {
        if !self.is_running() {
            return Err(AudioError::Runtime(
                "cannot load a kit while audio is not running".into(),
            ));
        }
        // Return any banks the engine has already retired so repeated kit
        // loads cannot fill the retire path.
        self.drain_retired_banks();
        // Decode/resample before touching the engine, so a bad kit leaves the
        // current sound untouched.
        let (bank, pads, names) = build_bank_from_kit(kit, kit_dir, self.sample_rate)?;
        if bank.is_empty() {
            return Err(AudioError::Runtime(format!(
                "kit {:?} has no playable samples",
                kit.metadata.name
            )));
        }
        let handle_present = match self.handle.lock() {
            Ok(slot) => slot.is_some(),
            Err(_) => false,
        };
        if !handle_present {
            return Err(AudioError::Runtime(
                "audio engine is not available for a kit swap".into(),
            ));
        }

        // A bank swap waits until the voice pool is idle. Fade any running
        // voices and wait briefly so a trigger immediately after this call is
        // not dropped by the pending-swap guard.
        if self.active_voices() > 0 {
            if let Ok(mut slot) = self.handle.lock()
                && let Some(handle) = slot.as_mut()
            {
                let _ = handle.panic();
            }
            let deadline = Instant::now() + Duration::from_millis(500);
            while self.active_voices() > 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            if self.active_voices() > 0 {
                return Err(AudioError::Runtime(
                    "active voices did not stop for the kit swap".into(),
                ));
            }
        }

        let mut slot = self
            .handle
            .lock()
            .map_err(|_| AudioError::Runtime("audio engine handle lock is poisoned".into()))?;
        let Some(handle) = slot.as_mut() else {
            return Err(AudioError::Runtime(
                "audio engine is not available for a kit swap".into(),
            ));
        };
        // One swap + six commands per pad. Refuse before sending anything if
        // the queue cannot take the whole kit, so a failed load cannot leave a
        // half-applied kit.
        let needed = 1 + PAD_COUNT * 6;
        if handle
            .commands
            .capacity()
            .saturating_sub(handle.commands.len())
            < needed
        {
            return Err(AudioError::Runtime(
                "command queue is too full for a complete kit swap".into(),
            ));
        }
        handle
            .send(Command::SwapSampleBank(Arc::clone(&bank)))
            .map_err(|_| AudioError::Runtime("command queue is full while swapping kit".into()))?;
        for (index, settings) in pads.iter().enumerate() {
            let pad = PadId::from_raw_unchecked(index as u8);
            if let Some(sample_slot) = settings.sample_slot {
                handle
                    .send(Command::SetPadSample {
                        pad,
                        slot: sample_slot,
                    })
                    .map_err(|_| {
                        AudioError::Runtime("command queue is full while loading kit".into())
                    })?;
            } else {
                handle.send(Command::ClearPadSample { pad }).map_err(|_| {
                    AudioError::Runtime("command queue is full while loading kit".into())
                })?;
            }
            handle
                .send(Command::SetPadPlayback {
                    pad,
                    playback: settings.playback,
                })
                .map_err(|_| {
                    AudioError::Runtime("command queue is full while loading kit".into())
                })?;
            handle
                .send(Command::SetPadGain {
                    pad,
                    gain: settings.gain,
                })
                .map_err(|_| {
                    AudioError::Runtime("command queue is full while loading kit".into())
                })?;
            handle
                .send(Command::SetPadPan {
                    pad,
                    pan: settings.pan,
                })
                .map_err(|_| {
                    AudioError::Runtime("command queue is full while loading kit".into())
                })?;
            handle
                .send(Command::SetPadTune {
                    pad,
                    cents: settings.tune_cents,
                })
                .map_err(|_| {
                    AudioError::Runtime("command queue is full while loading kit".into())
                })?;
            handle
                .send(Command::SetPadChokeGroup {
                    pad,
                    group: settings.choke_group,
                })
                .map_err(|_| {
                    AudioError::Runtime("command queue is full while loading kit".into())
                })?;
        }
        drop(slot);
        {
            let mut state = self.kit_state.lock().map_err(|_| {
                AudioError::Runtime("kit state lock is poisoned after a swap".into())
            })?;
            state.bank = Arc::clone(&bank);
            state.pads = pads;
        }
        self.pad_names = names;
        Ok(())
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

    /// Drop sample banks the engine has already retired. Callers must be on a
    /// non-real-time thread; this is deliberately done from `tick` and
    /// `load_kit`, never from the audio callback.
    fn drain_retired_banks(&self) {
        if let Ok(mut slot) = self.handle.lock()
            && let Some(handle) = slot.as_mut()
        {
            let _ = handle.collect_retired_banks();
        }
    }

    /// Poll the audio system. Call this from the UI at ~10 Hz.
    pub fn tick(&mut self) -> Option<AudioSystemEvent> {
        self.drain_retired_banks();
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

/// Decode the embedded demo WAVs into a temporary kit folder, then reuse the
/// same mapping as an imported kit.
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
    }
    let result = build_bank_from_kit(&kit, &temp, device_rate);
    let _ = std::fs::remove_dir_all(&temp);
    result
}

/// Build a bank and pad settings from any kit layout: `kit_dir` is the folder
/// that `SampleRef::resolve` joins to. The first velocity layer of each pad is
/// used for the MVP; samples are decoded and resampled on this (non-audio)
/// thread.
fn build_bank_from_kit(
    kit: &Kit,
    kit_dir: &Path,
    device_rate: u32,
) -> Result<DemoBank, AudioError> {
    let mut decoded: HashMap<PathBuf, SampleBuffer> = HashMap::new();
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
        let path = sample_ref.resolve(kit_dir);
        let sample = if let Some(cached) = decoded.get(&path) {
            cached.clone()
        } else {
            let decoded_audio = decode_file(&path).map_err(|error| {
                AudioError::Runtime(format!(
                    "could not decode kit sample {}: {error}",
                    path.display()
                ))
            })?;
            let prepared =
                prepare_for_device(&decoded_audio.audio, device_rate, false).map_err(|error| {
                    AudioError::Runtime(format!(
                        "could not resample kit sample {} to {device_rate} Hz: {error}",
                        path.display()
                    ))
                })?;
            let buffer = prepared.into_sample_buffer().map_err(|error| {
                AudioError::Runtime(format!(
                    "kit sample {} is not a valid playback buffer: {error}",
                    path.display()
                ))
            })?;
            decoded.insert(path.clone(), buffer.clone());
            buffer
        };

        let slot = bank.push(Arc::new(sample));
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
    use drumpad_model::kit::RelativeSamplePath;
    use std::time::Instant;

    fn write_demo_samples(dir: &Path) {
        for (relative, bytes) in DEMO_SAMPLES {
            let path = dir.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, bytes).unwrap();
        }
    }

    fn demo_kit_named(name: &str) -> Kit {
        let mut kit = demo_kit().unwrap();
        if let Some(pad) = kit.pads.iter_mut().find(|pad| pad.id.index() == 0) {
            pad.name = Some(name.to_owned());
        }
        kit
    }

    fn start_fake_runtime() -> MvpAudioRuntime {
        MvpAudioRuntime::start_with_backend(
            Box::new(FakeBackend::standard().with_period(Duration::from_millis(2))),
            None,
            EngineConfig {
                sample_rate: MVP_SAMPLE_RATE,
                voice_limit: 64,
                ..EngineConfig::default()
            },
        )
        .expect("runtime")
    }

    fn wait_for_voice(runtime: &MvpAudioRuntime, active: bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while (runtime.active_voices() > 0) != active && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            runtime.active_voices() > 0,
            active,
            "voice state did not settle"
        );
    }

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
    fn load_kit_swaps_bank_names_and_keeps_playing() {
        let dir = demo_temp_dir();
        write_demo_samples(&dir);
        let mut runtime = start_fake_runtime();

        let first = demo_kit_named("Imported Kick");
        runtime.load_kit(&first, &dir).expect("first kit");
        assert_eq!(runtime.pad_names()[0].as_deref(), Some("Imported Kick"));
        runtime.trigger(0, 100);
        wait_for_voice(&runtime, true);
        assert!(
            runtime.engine_snapshot().is_some_and(|engine| engine
                .meter
                .peak_left
                .max(engine.meter.peak_right)
                > 0.0)
        );

        let second = demo_kit_named("Second Kick");
        runtime.load_kit(&second, &dir).expect("second kit");
        assert_eq!(runtime.pad_names()[0].as_deref(), Some("Second Kick"));
        runtime.trigger(1, 100);
        wait_for_voice(&runtime, true);
        runtime.panic();
        wait_for_voice(&runtime, false);
        runtime.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_kit_rejects_missing_samples_and_empty_banks() {
        let dir = demo_temp_dir();
        write_demo_samples(&dir);
        let mut runtime = start_fake_runtime();

        let mut broken = demo_kit_named("Broken");
        let pad = broken
            .pads
            .iter_mut()
            .find(|pad| pad.id.index() == 0)
            .unwrap();
        pad.layers[0].sample.as_mut().unwrap().path =
            RelativeSamplePath::new("samples/missing.wav").unwrap();
        let error = runtime.load_kit(&broken, &dir).unwrap_err();
        assert!(matches!(error, AudioError::Runtime(_)), "{error}");

        let mut empty = demo_kit_named("Empty");
        for pad in &mut empty.pads {
            pad.layers.clear();
        }
        let error = runtime.load_kit(&empty, &dir).unwrap_err();
        assert!(matches!(error, AudioError::Runtime(_)), "{error}");

        runtime.stop();
        let error = runtime
            .load_kit(&demo_kit_named("Stopped"), &dir)
            .unwrap_err();
        assert!(matches!(error, AudioError::Runtime(_)), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_kit_clears_pads_without_samples() {
        let dir = demo_temp_dir();
        write_demo_samples(&dir);
        let mut runtime = start_fake_runtime();

        let mut sparse = demo_kit_named("Sparse");
        for pad in &mut sparse.pads {
            if pad.id.index() != 0 {
                pad.layers.clear();
            }
        }
        runtime.load_kit(&sparse, &dir).expect("sparse kit");
        runtime.trigger(1, 100);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            runtime.active_voices(),
            0,
            "a pad without a sample played the stale bank slot"
        );

        runtime.trigger(0, 100);
        wait_for_voice(&runtime, true);
        runtime.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loaded_kit_survives_a_device_loss_reconnect() {
        let dir = demo_temp_dir();
        write_demo_samples(&dir);
        let backend = FakeBackend::standard().with_period(Duration::from_millis(2));
        let control = backend.clone();
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

        let mut sparse = demo_kit_named("Sparse reconnect");
        for pad in &mut sparse.pads {
            if pad.id.index() != 0 {
                pad.layers.clear();
            }
        }
        runtime.load_kit(&sparse, &dir).expect("sparse kit");

        control.set_stream_failure(true);
        let deadline = Instant::now() + Duration::from_secs(2);
        while runtime.state().is_running() && Instant::now() < deadline {
            runtime.tick();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!runtime.state().is_running(), "device loss not observed");

        control.set_stream_failure(false);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !runtime.state().is_running() && Instant::now() < deadline {
            runtime.tick();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(runtime.state().is_running(), "runtime did not reconnect");

        // Pad 1 was cleared by the imported kit. A reconnect must not bring
        // back the startup demo bank.
        runtime.trigger(1, 100);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            runtime.active_voices(),
            0,
            "reconnect restored the startup demo bank instead of the loaded kit"
        );

        runtime.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_load_kit_swaps_do_not_stall_the_retire_path() {
        let dir = demo_temp_dir();
        write_demo_samples(&dir);
        let mut runtime = start_fake_runtime();
        let full = demo_kit_named("Full");
        let mut sparse = demo_kit_named("Sparse");
        for pad in &mut sparse.pads {
            if pad.id.index() != 0 {
                pad.layers.clear();
            }
        }

        // More swaps than the engine's retire queue (8) plus overflow slots
        // (16). Each load should drain the previous retired bank.
        for _ in 0..30 {
            runtime.load_kit(&full, &dir).expect("full kit");
            std::thread::sleep(Duration::from_millis(3));
        }
        runtime.load_kit(&sparse, &dir).expect("sparse kit");
        runtime.trigger(0, 100);
        let deadline = Instant::now() + Duration::from_secs(1);
        while runtime.active_voices() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            runtime.active_voices() > 0,
            "final kit swap never installed after repeated loads"
        );

        runtime.stop();
        let _ = std::fs::remove_dir_all(&dir);
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
    fn runtime_prefers_exclusive_mode_when_the_device_offers_it() {
        let runtime = start_fake_runtime();
        let format = runtime.format().expect("negotiated format");
        assert_eq!(
            format.share_mode,
            crate::ShareMode::Exclusive,
            "the MVP default must request the low-latency exclusive stream"
        );
        assert!(!format.fell_back);
        assert_eq!(format.callback_period_frames, MVP_BLOCK_FRAMES as u32);
    }

    #[test]
    fn runtime_falls_back_to_shared_when_exclusive_is_unavailable() {
        // A device that only exposes a shared float configuration, like a busy
        // endpoint or a driver that rejects integer exclusive formats.
        let backend = FakeBackend::standard().with_configs(vec![crate::backend::SupportedConfig {
            sample_rate: 48_000,
            channels: 2,
            sample_format: crate::SampleFormat::F32,
            share_mode: crate::ShareMode::Shared,
            min_buffer_frames: 64,
            max_buffer_frames: 2048,
        }]);
        let mut runtime = MvpAudioRuntime::start_with_backend(
            Box::new(backend),
            None,
            EngineConfig {
                sample_rate: MVP_SAMPLE_RATE,
                voice_limit: 64,
                ..EngineConfig::default()
            },
        )
        .expect("runtime must still start on the shared fallback");
        let format = runtime.format().expect("negotiated format");
        assert_eq!(format.share_mode, crate::ShareMode::Shared);
        assert!(format.fell_back);
        assert!(
            format.fallback_reason.is_some(),
            "the fallback reason must be visible to the UI"
        );

        runtime.trigger(0, 100);
        let deadline = Instant::now() + Duration::from_secs(2);
        while runtime.active_voices() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            runtime.active_voices() > 0,
            "the shared fallback must still play pads"
        );
        runtime.stop();
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
