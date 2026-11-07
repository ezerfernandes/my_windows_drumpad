//! Deterministic offline rendering.
//!
//! The renderer drives the real [`drumpad_engine::mixer::Engine`] block by
//! block on a single thread. There is no audio device, no wall clock, and no
//! randomness: a sequence plus a config always produces the same interleaved
//! stereo buffer. Trigger events are quantized to the next block boundary,
//! which is exactly how the input thread timestamps them at run time
//! (see [`crate::sequence::SequenceEvent::scheduled_frame`]).

use std::error::Error;
use std::fmt;
use std::path::Path;

use drumpad_engine::mixer::{Engine, EngineConfig, EngineHandle};
use drumpad_engine::queue::Command;
use drumpad_engine::voice::SampleBuffer;
use drumpad_input::trigger::TriggerSource;
use drumpad_model::ids::{PadId, Velocity};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fixture::{self, FixtureBank, PadFixture};
use crate::sequence::{Action, Sequence};

/// Default render sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;

/// Default render block size in frames.
pub const DEFAULT_BLOCK_FRAMES: usize = 128;

/// Default maximum voice allocation.
pub const DEFAULT_MAX_VOICES: usize = 256;

/// Default usable voice count.
pub const DEFAULT_VOICE_LIMIT: usize = 64;

/// Default tail after the last scripted event, in frames (one second).
pub const DEFAULT_TAIL_FRAMES: u64 = 48_000;

/// Longest accepted tail, in frames (ten minutes).
pub const MAX_TAIL_FRAMES: u64 = 28_800_000;

/// Offline render configuration. Serializable so a fixture directory can ship
/// the exact config used for a golden render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderConfig {
    /// Stream sample rate in Hz.
    pub sample_rate: u32,
    /// Block size in frames.
    pub block_frames: usize,
    /// Allocated voice ceiling.
    pub max_voices: usize,
    /// Usable voice count.
    pub voice_limit: usize,
    /// Whether the master limiter is enabled.
    pub limiter: bool,
    /// Silence rendered after the last event.
    pub tail_frames: u64,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            block_frames: DEFAULT_BLOCK_FRAMES,
            max_voices: DEFAULT_MAX_VOICES,
            voice_limit: DEFAULT_VOICE_LIMIT,
            limiter: true,
            tail_frames: DEFAULT_TAIL_FRAMES,
        }
    }
}

impl RenderConfig {
    /// Validate the ranges the engine accepts.
    pub fn validate(&self) -> Result<(), RenderError> {
        if self.sample_rate < 8_000 {
            return Err(RenderError::Config(format!(
                "sample_rate {} is below the engine minimum 8000",
                self.sample_rate
            )));
        }
        if self.block_frames == 0 || self.block_frames > 4_096 {
            return Err(RenderError::Config(format!(
                "block_frames {} is outside 1..=4096",
                self.block_frames
            )));
        }
        if !(16..=256).contains(&self.max_voices) {
            return Err(RenderError::Config(format!(
                "max_voices {} is outside 16..=256",
                self.max_voices
            )));
        }
        if self.voice_limit < 16 || self.voice_limit > self.max_voices {
            return Err(RenderError::Config(format!(
                "voice_limit {} is outside 16..={}",
                self.voice_limit, self.max_voices
            )));
        }
        if self.tail_frames > MAX_TAIL_FRAMES {
            return Err(RenderError::Config(format!(
                "tail_frames {} exceeds the {} frame limit",
                self.tail_frames, MAX_TAIL_FRAMES
            )));
        }
        Ok(())
    }
}

/// Summary statistics for a rendered buffer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderStats {
    /// Rendered frames.
    pub frames: u64,
    /// Largest absolute sample.
    pub peak: f32,
    /// Root mean square across both channels.
    pub rms: f32,
    /// Peak in dBFS, floored at -120.
    pub peak_db: f32,
    /// RMS in dBFS, floored at -120.
    pub rms_db: f32,
    /// Number of NaN or infinite samples.
    pub non_finite: usize,
    /// First frame with a non-zero sample, if any.
    pub first_non_zero_frame: Option<u64>,
    /// Last frame with a non-zero sample, if any.
    pub last_non_zero_frame: Option<u64>,
    /// SHA-256 of the interleaved `f32` little-endian output.
    pub output_sha256: String,
}

impl RenderStats {
    /// Analyze an interleaved stereo buffer.
    pub fn analyze(output: &[f32], frames: u64) -> Self {
        let mut peak = 0.0f32;
        let mut sum = 0.0f64;
        let mut non_finite = 0usize;
        let mut first_non_zero = None;
        let mut last_non_zero = None;
        for (index, sample) in output.iter().enumerate() {
            if !sample.is_finite() {
                non_finite += 1;
                continue;
            }
            let magnitude = sample.abs();
            if magnitude > peak {
                peak = magnitude;
            }
            sum += f64::from(*sample) * f64::from(*sample);
            if *sample != 0.0 {
                let frame = (index / 2) as u64;
                if first_non_zero.is_none() {
                    first_non_zero = Some(frame);
                }
                last_non_zero = Some(frame);
            }
        }
        let count = output.len().max(1) as f64;
        let rms = (sum / count).sqrt() as f32;
        let mut hasher = Sha256::new();
        hasher.update((output.len() as u64).to_le_bytes());
        for sample in output {
            hasher.update(sample.to_bits().to_le_bytes());
        }
        Self {
            frames,
            peak,
            rms,
            peak_db: linear_to_db(peak),
            rms_db: linear_to_db(rms),
            non_finite,
            first_non_zero_frame: first_non_zero,
            last_non_zero_frame: last_non_zero,
            output_sha256: format!("{:x}", hasher.finalize()),
        }
    }
}

/// Linear amplitude to dBFS, floored at -120 dB.
pub fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        -120.0
    } else {
        20.0 * linear.log10()
    }
}

/// Active-voice counts captured after one rendered block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceTraceEntry {
    /// Block index.
    pub block: usize,
    /// First frame of the next block (the engine playhead after the block).
    pub frame: u64,
    /// Total active voices.
    pub active: u16,
    /// Active voices per pad.
    pub per_pad: [u16; 16],
}

impl VoiceTraceEntry {
    /// Active voices playing `pad`.
    pub fn pad_count(&self, pad: usize) -> u16 {
        self.per_pad.get(pad).copied().unwrap_or(0)
    }
}

/// The result of one offline render.
#[derive(Debug, Clone)]
pub struct OfflineRender {
    /// Configuration used.
    pub config: RenderConfig,
    /// Rendered frames (not samples).
    pub frames: u64,
    /// Interleaved stereo output, `frames * 2` samples.
    pub output: Vec<f32>,
    /// Active voice counts sampled after each block.
    pub voice_trace: Vec<VoiceTraceEntry>,
    /// Note-on count per pad.
    pub trigger_count: [u32; 16],
    /// Summary statistics.
    pub stats: RenderStats,
}

impl OfflineRender {
    /// Write the output as a 32-bit float stereo WAV.
    pub fn write_wav(&self, path: impl AsRef<Path>) -> Result<(), RenderError> {
        let path = path.as_ref();
        let buffer = SampleBuffer::new(self.output.clone(), 2, self.config.sample_rate)
            .map_err(|error| RenderError::Config(error.to_string()))?;
        fixture::write_wav(path, &buffer).map_err(|error| RenderError::Fixture(error.to_string()))
    }

    /// True when no voice remains active at the end.
    pub fn no_stuck_voices(&self) -> bool {
        self.voice_trace
            .last()
            .map(|entry| entry.active)
            .unwrap_or(0)
            == 0
    }

    /// Active voice count after a given frame.
    pub fn voices_at(&self, frame: u64) -> u16 {
        let block = (frame / self.config.block_frames as u64) as usize;
        self.voice_trace
            .get(block.min(self.voice_trace.len().saturating_sub(1)))
            .map(|entry| entry.active)
            .unwrap_or(0)
    }

    /// Active voices on `pad` after a given frame.
    pub fn pad_voices_at(&self, frame: u64, pad: usize) -> u16 {
        let block = (frame / self.config.block_frames as u64) as usize;
        self.voice_trace
            .get(block.min(self.voice_trace.len().saturating_sub(1)))
            .map(|entry| entry.pad_count(pad))
            .unwrap_or(0)
    }
}

/// Errors from offline rendering.
#[derive(Debug)]
pub enum RenderError {
    /// Invalid configuration.
    Config(String),
    /// The trigger or command queue rejected an event.
    Queue(String),
    /// A fixture could not be built or written.
    Fixture(String),
    /// The engine reported no installation of the sample bank.
    Bank(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message) => write!(formatter, "invalid render config: {message}"),
            Self::Queue(message) => write!(formatter, "engine queue rejected an event: {message}"),
            Self::Fixture(message) => write!(formatter, "fixture error: {message}"),
            Self::Bank(message) => write!(formatter, "sample bank error: {message}"),
        }
    }
}

impl Error for RenderError {}

/// A live offline engine with the standard fixture bank installed.
pub struct OfflineSession {
    engine: Engine,
    handle: EngineHandle,
    config: RenderConfig,
    base_frame: u64,
    playhead: u64,
}

impl OfflineSession {
    /// Create a session and install the standard fixture bank.
    pub fn new(config: RenderConfig) -> Result<Self, RenderError> {
        Self::with_bank(config, FixtureBank::standard())
    }

    /// Create a session with a caller-provided fixture bank.
    pub fn with_bank(config: RenderConfig, fixture_bank: FixtureBank) -> Result<Self, RenderError> {
        config.validate()?;
        let engine_config = EngineConfig {
            sample_rate: config.sample_rate,
            max_voices: config.max_voices,
            voice_limit: config.voice_limit,
            ..EngineConfig::default()
        };
        let (mut engine, mut handle) = Engine::new(engine_config);
        if !config.limiter {
            handle
                .send(Command::SetLimiterEnabled(false))
                .map_err(|command| RenderError::Queue(format!("{command:?}")))?;
        }
        handle
            .send(Command::SwapSampleBank(std::sync::Arc::new(
                fixture_bank.bank,
            )))
            .map_err(|command| RenderError::Queue(format!("{command:?}")))?;
        // One block installs the bank; with no active voices the swap is
        // immediate at the end of the block.
        let mut silence = vec![0.0f32; config.block_frames * 2];
        engine.process(&mut silence, config.block_frames);
        for (pad, settings) in fixture_bank.pads.iter().enumerate() {
            apply_pad(&mut handle, pad as u8, *settings)?;
        }
        // One block applies every pad command before the first trigger.
        let mut silence = vec![0.0f32; config.block_frames * 2];
        engine.process(&mut silence, config.block_frames);
        let base_frame = config.block_frames as u64 * 2;
        Ok(Self {
            engine,
            handle,
            config,
            base_frame,
            playhead: base_frame,
        })
    }

    /// Engine frames consumed before the first scripted frame. Input
    /// timestamps must be offset by this value so they land on the intended
    /// render frame.
    pub fn base_frame(&self) -> u64 {
        self.base_frame
    }

    /// Convert a logical render frame into the engine's microsecond input
    /// clock.
    pub fn frame_to_us(&self, frame: u64) -> u64 {
        self.base_frame
            .saturating_add(frame)
            .saturating_mul(1_000_000)
            / u64::from(self.config.sample_rate.max(1))
    }

    /// Mutable access to the engine handle, for harnesses that inject input
    /// events themselves.
    pub fn handle_mut(&mut self) -> &mut EngineHandle {
        &mut self.handle
    }

    /// Immutable access to the engine handle.
    pub fn handle(&self) -> &EngineHandle {
        &self.handle
    }

    /// Immutable access to the engine.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Render exactly one block and return it as interleaved stereo.
    pub fn process_block(&mut self) -> Vec<f32> {
        let mut out = vec![0.0f32; self.config.block_frames * 2];
        self.process_into(&mut out);
        out
    }

    /// Render exactly one block into a caller-provided buffer.
    ///
    /// `out` must hold at least `block_frames * 2` samples; extra samples are
    /// left untouched. Allocation-free, for benchmarks and tight loops.
    pub fn process_into(&mut self, out: &mut [f32]) {
        self.engine.process(out, self.config.block_frames);
        self.playhead = self
            .playhead
            .saturating_add(self.config.block_frames as u64);
    }

    /// Active voice count per pad right now.
    pub fn active_pad_counts(&self) -> [u16; 16] {
        let mut counts = [0u16; 16];
        for index in 0..self.config.max_voices {
            if let Some(snapshot) = self.engine.voice_snapshot(index)
                && snapshot.active
                && let Some(count) = counts.get_mut(snapshot.pad.index())
            {
                *count = count.saturating_add(1);
            }
        }
        counts
    }

    /// Total active voices right now.
    pub fn active_voices(&self) -> usize {
        self.engine.active_voice_count()
    }

    /// Configuration in force.
    pub fn config(&self) -> &RenderConfig {
        &self.config
    }

    /// Render a sequence to completion plus the configured tail.
    pub fn render(&mut self, sequence: &Sequence) -> Result<OfflineRender, RenderError> {
        let total_frames = sequence
            .last_scheduled_frame(self.config.block_frames)
            .saturating_add(self.config.tail_frames)
            .max(self.config.block_frames as u64);
        let blocks = total_frames.div_ceil(self.config.block_frames as u64) as usize;
        let mut output = Vec::with_capacity(blocks * self.config.block_frames * 2);
        let mut voice_trace = Vec::with_capacity(blocks);
        let mut trigger_count = [0u32; 16];
        let mut cursor = 0usize;
        for block in 0..blocks {
            let block_start = (block as u64) * self.config.block_frames as u64;
            for event in sequence.due(&mut cursor, block_start) {
                match event.action {
                    Action::NoteOn { pad, velocity } => {
                        let pad_id = PadId::from_raw_unchecked(pad);
                        let velocity = Velocity::new(velocity).map_err(|error| {
                            RenderError::Queue(format!("invalid velocity: {error}"))
                        })?;
                        self.handle
                            .note_on(pad_id, velocity, TriggerSource::UiPreview)
                            .map_err(|error| RenderError::Queue(format!("{error:?}")))?;
                        trigger_count[pad as usize] += 1;
                    }
                    Action::NoteOff { pad } => {
                        self.handle
                            .note_off(PadId::from_raw_unchecked(pad), TriggerSource::UiPreview)
                            .map_err(|error| RenderError::Queue(format!("{error:?}")))?;
                    }
                    Action::AllNotesOff => {
                        self.handle
                            .all_notes_off()
                            .map_err(|command| RenderError::Queue(format!("{command:?}")))?;
                    }
                    Action::Panic => {
                        self.handle
                            .panic()
                            .map_err(|command| RenderError::Queue(format!("{command:?}")))?;
                    }
                }
            }
            let mut block_output = vec![0.0f32; self.config.block_frames * 2];
            self.engine
                .process(&mut block_output, self.config.block_frames);
            output.extend_from_slice(&block_output);
            let mut per_pad = [0u16; 16];
            let mut active = 0u16;
            for index in 0..self.config.max_voices {
                if let Some(snapshot) = self.engine.voice_snapshot(index)
                    && snapshot.active
                {
                    active = active.saturating_add(1);
                    let pad = snapshot.pad.index();
                    if let Some(count) = per_pad.get_mut(pad) {
                        *count = count.saturating_add(1);
                    }
                }
            }
            voice_trace.push(VoiceTraceEntry {
                block,
                frame: (block as u64 + 1) * self.config.block_frames as u64,
                active,
                per_pad,
            });
        }
        // The engine playhead is tracked so callers can schedule a second
        // render on the same session without replaying time.
        self.playhead = self.playhead.saturating_add(total_frames);
        let stats = RenderStats::analyze(&output, output.len() as u64 / 2);
        Ok(OfflineRender {
            config: self.config,
            frames: stats.frames,
            output,
            voice_trace,
            trigger_count,
            stats,
        })
    }

    /// Current engine playhead in frames.
    pub fn playhead(&self) -> u64 {
        self.playhead
    }
}

fn apply_pad(handle: &mut EngineHandle, pad: u8, settings: PadFixture) -> Result<(), RenderError> {
    let pad_id = PadId::from_raw_unchecked(pad);
    for command in [
        Command::SetPadSample {
            pad: pad_id,
            slot: settings.slot,
        },
        Command::SetPadPlayback {
            pad: pad_id,
            playback: settings.playback,
        },
        Command::SetPadGain {
            pad: pad_id,
            gain: settings.gain,
        },
        Command::SetPadPan {
            pad: pad_id,
            pan: settings.pan,
        },
        Command::SetPadTune {
            pad: pad_id,
            cents: settings.tune_cents,
        },
        Command::SetPadChokeGroup {
            pad: pad_id,
            group: settings.choke_group,
        },
    ] {
        handle
            .send(command)
            .map_err(|command| RenderError::Queue(format!("{command:?}")))?;
    }
    Ok(())
}

/// Render a sequence with a fresh session.
pub fn render(sequence: &Sequence, config: RenderConfig) -> Result<OfflineRender, RenderError> {
    let mut session = OfflineSession::new(config)?;
    session.render(sequence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequence::Sequence;

    fn config(block_frames: usize) -> RenderConfig {
        RenderConfig {
            block_frames,
            tail_frames: 4_800,
            ..RenderConfig::default()
        }
    }

    #[test]
    fn identical_sequences_render_bit_identically() {
        let sequence = Sequence::parse("0 note_on 0 100\n960 note_on 2 80\n2400 note_off 2\n")
            .expect("sequence");
        let first = render(&sequence, config(128)).expect("first");
        let second = render(&sequence, config(128)).expect("second");
        assert_eq!(first.output, second.output);
        assert_eq!(first.stats.output_sha256, second.stats.output_sha256);
    }

    #[test]
    fn block_size_changes_quantization_but_not_validity() {
        let sequence = Sequence::parse("1 note_on 0 100\n").expect("sequence");
        let at_128 = render(&sequence, config(128)).expect("128");
        let at_64 = render(&sequence, config(64)).expect("64");
        assert_eq!(
            at_128.stats.first_non_zero_frame,
            Some(129),
            "event at frame 1 is applied at block 128 and the 24-frame start fade makes sample 128 zero"
        );
        assert_eq!(at_64.stats.first_non_zero_frame, Some(65));
        assert!(at_128.stats.peak > 0.0);
        assert!(at_64.stats.peak > 0.0);
    }

    #[test]
    fn empty_sequence_renders_only_silence() {
        let sequence = Sequence::parse("# empty\n").expect("sequence");
        let rendered = render(&sequence, config(128)).expect("render");
        assert_eq!(rendered.stats.peak, 0.0);
        assert_eq!(rendered.stats.first_non_zero_frame, None);
        assert_eq!(rendered.trigger_count, [0u32; 16]);
        assert!(rendered.no_stuck_voices());
    }

    #[test]
    fn trigger_counts_and_voice_trace_are_deterministic() {
        let sequence = Sequence::parse(
            "0 note_on 0 100\n0 note_on 1 100\n0 note_on 2 100\n128 note_on 3 100\n",
        )
        .expect("sequence");
        let rendered = render(&sequence, config(128)).expect("render");
        assert_eq!(rendered.trigger_count[0], 1);
        assert_eq!(rendered.trigger_count[1], 1);
        assert_eq!(rendered.trigger_count[2], 1);
        assert_eq!(rendered.trigger_count[3], 1);
        assert_eq!(rendered.trigger_count[4], 0);
        assert!(!rendered.voice_trace.is_empty());
        assert!(
            rendered
                .voice_trace
                .iter()
                .map(|entry| entry.active)
                .max()
                .unwrap_or(0)
                >= 3
        );
    }

    #[test]
    fn limiter_ceiling_never_reaches_full_scale() {
        let mut sequence_text = String::new();
        for pad in 5..16 {
            sequence_text.push_str(&format!("0 note_on {pad} 127\n"));
        }
        let sequence = Sequence::parse(&sequence_text).expect("sequence");
        let rendered = render(&sequence, config(128)).expect("render");
        assert_eq!(rendered.stats.non_finite, 0);
        // The engine's final clip guard clamps at exactly 1.0, which is 0 dBFS
        // and therefore still inside the design's "never exceeds 0 dBFS"
        // guarantee. The stricter "never reaches 1.0" gate lives in the
        // E11-T03 golden test so it can be reported as an engine finding.
        assert!(
            rendered.stats.peak <= 1.0,
            "peak {} exceeded full scale",
            rendered.stats.peak
        );
    }

    #[test]
    fn invalid_config_is_rejected() {
        let bad = RenderConfig {
            block_frames: 0,
            ..RenderConfig::default()
        };
        assert!(matches!(bad.validate(), Err(RenderError::Config(_))));
        let bad = RenderConfig {
            voice_limit: 300,
            ..RenderConfig::default()
        };
        assert!(matches!(bad.validate(), Err(RenderError::Config(_))));
    }
}
