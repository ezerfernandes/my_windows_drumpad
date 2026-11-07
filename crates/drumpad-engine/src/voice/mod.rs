//! Immutable sample buffers, the per-voice playback state machine, and
//! loop/interpolation primitives used by the real-time mixer.
//!
//! Nothing in this module allocates or locks while a voice is rendering.
//! Buffers are built off the audio thread and published as an immutable
//! [`SampleBank`]. A voice only ever holds a slot index into the bank, never an
//! owned pointer, so dropping a voice can never free memory on the audio
//! thread.

pub use drumpad_samples::{SampleBuffer, SampleBufferError};

use drumpad_model::ids::PadId;

/// Number of channels supported by the mixer.
pub const MAX_SAMPLE_CHANNELS: u8 = 2;

/// Length of the automatic de-click fade at the start of a one-shot, in
/// seconds. Half a millisecond is short enough not to soften a drum transient
/// but long enough to remove the DC step of a non-zero first sample.
pub const START_FADE_SECS: f32 = 0.0005;

/// Length of the automatic de-click fade at the end of a one-shot.
pub const END_FADE_SECS: f32 = 0.0005;

/// A published set of samples addressable by a stable slot index.
///
/// The bank is built off the audio thread (after decoding/resampling) and
/// wrapped in an `Arc`. The audio thread only reads through the bank; swapping
/// a bank publishes a new `Arc` through the command queue and retires the old
/// one back to the UI thread instead of dropping it in the callback.
///
/// `SampleBuffer` is the canonical type from `drumpad-samples`; it already
/// holds an `Arc<[f32]>`, so a bank stores the buffers directly. Cloning a
/// buffer is cheap and only happens off the audio thread.
#[derive(Debug, Default, Clone)]
pub struct SampleBank {
    slots: Vec<std::sync::Arc<SampleBuffer>>,
}

impl SampleBank {
    /// Create an empty bank.
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// Append a sample and return its slot index. Off-thread only.
    pub fn push(&mut self, sample: std::sync::Arc<SampleBuffer>) -> u16 {
        let slot = self.slots.len();
        self.slots.push(sample);
        assert!(slot <= u16::MAX as usize, "sample bank slot overflow");
        slot as u16
    }

    /// Replace a slot. Off-thread only.
    pub fn set(&mut self, slot: u16, sample: std::sync::Arc<SampleBuffer>) {
        if let Some(entry) = self.slots.get_mut(slot as usize) {
            *entry = sample;
        } else {
            self.push(sample);
        }
    }

    /// Look up a slot.
    #[inline]
    pub fn get(&self, slot: u16) -> Option<&SampleBuffer> {
        self.slots.get(slot as usize).map(|sample| sample.as_ref())
    }

    /// Shared handle for a slot, used by non-real-time callers.
    pub fn get_shared(&self, slot: u16) -> Option<&std::sync::Arc<SampleBuffer>> {
        self.slots.get(slot as usize)
    }

    /// Number of slots.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// True when no sample has been published.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

/// Read one channel of one frame. Mono buffers return the same value for both
/// channels. Kept here because `drumpad-samples` intentionally exposes only
/// `data()` and the channel count.
#[inline]
pub(crate) fn sample_value(sample: &SampleBuffer, frame: usize, channel: usize) -> f32 {
    let data = sample.data();
    match sample.channels() {
        2 => data[frame * 2 + channel.min(1)],
        _ => data[frame],
    }
}

/// How a voice should sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayMode {
    /// Plays once to the end of the buffer.
    #[default]
    OneShot,
    /// Plays until a note-off, then releases with a short fade.
    Gate,
    /// Loops between the loop points until stopped or choked.
    Loop,
}

/// Sample-accurate loop bounds in *native sample frames*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoopPoints {
    /// First frame of the loop body.
    pub start: u32,
    /// One past the last frame of the loop body.
    pub end: u32,
    /// Crossfade length in frames. Zero disables crossfading.
    pub crossfade: u32,
}

impl LoopPoints {
    /// A loop covering the whole buffer with no crossfade.
    pub fn full(frames: usize) -> Self {
        Self {
            start: 0,
            end: frames.min(u32::MAX as usize) as u32,
            crossfade: 0,
        }
    }

    /// Clamp the points into a valid region for `frames` and mode.
    pub fn sanitized(self, frames: usize, mode: PlayMode) -> Self {
        let frames = frames.min(u32::MAX as usize) as u32;
        if mode != PlayMode::Loop || frames == 0 {
            return Self::default();
        }
        let start = self.start.min(frames.saturating_sub(1));
        let mut end = self.end.clamp(start.saturating_add(1), frames);
        if end <= start {
            end = frames;
        }
        let len = end - start;
        let crossfade = self.crossfade.min(len / 2);
        Self {
            start,
            end,
            crossfade,
        }
    }

    /// True when the loop region has at least one frame.
    pub fn is_valid_for(&self, frames: usize) -> bool {
        self.end as usize <= frames && self.start < self.end
    }
}

/// Playback parameters captured when a voice starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Playback {
    /// Playback mode.
    pub mode: PlayMode,
    /// Loop points, only used by [`PlayMode::Loop`].
    pub loop_points: LoopPoints,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            mode: PlayMode::OneShot,
            loop_points: LoopPoints::default(),
        }
    }
}

/// Interpolation quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interpolation {
    /// Two-point linear interpolation, cheapest.
    Linear,
    /// Four-point Catmull-Rom cubic interpolation.
    #[default]
    Cubic,
}

/// Everything a voice needs to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceSpec {
    /// Pad that triggered the voice.
    pub pad: PadId,
    /// Slot in the sample bank.
    pub sample_slot: u16,
    /// Playback mode and loop points.
    pub playback: Playback,
    /// Tuning in cents. `1200` is one octave up.
    pub tune_cents: i32,
    /// MIDI-style velocity in `0..=127`.
    pub velocity: u8,
    /// Interpolation quality.
    pub interpolation: Interpolation,
}

impl VoiceSpec {
    /// A plain one-shot at full velocity.
    pub fn one_shot(sample_slot: u16) -> Self {
        Self {
            pad: PadId::FIRST,
            sample_slot,
            playback: Playback::default(),
            tune_cents: 0,
            velocity: 127,
            interpolation: Interpolation::default(),
        }
    }
}

/// Lifecycle state exposed for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoicePhase {
    /// Free for allocation.
    Idle,
    /// Producing samples.
    Playing,
    /// Fading out towards idle or a pending restart.
    Releasing,
}

/// A single preallocated voice.
///
/// The struct is `Copy`-free by design (it owns nothing on the heap), and
/// [`Voice::reset`] returns it to a fully idle, reusable state.
#[derive(Debug, Clone)]
pub struct Voice {
    active: bool,
    pad: u8,
    slot: u16,
    frames: u32,
    sample_rate: u32,
    source_channels: u8,
    pos: f64,
    inc: f64,
    velocity_gain: f32,
    mode: PlayMode,
    loops: LoopPoints,
    interpolation: Interpolation,
    fade_in_total: u32,
    fade_in_remaining: u32,
    fade_out_total: u32,
    fade_out_remaining: u32,
    stop_after_fade: bool,
    end_fade_frames: u32,
    pending: Option<VoiceSpec>,
    pending_order: u64,
    started_at: u64,
}

impl Default for Voice {
    fn default() -> Self {
        Self::new()
    }
}

impl Voice {
    /// Create an idle voice. No allocation happens here.
    pub const fn new() -> Self {
        Self {
            active: false,
            pad: 0,
            slot: 0,
            frames: 0,
            sample_rate: 0,
            source_channels: 1,
            pos: 0.0,
            inc: 1.0,
            velocity_gain: 0.0,
            mode: PlayMode::OneShot,
            loops: LoopPoints {
                start: 0,
                end: 0,
                crossfade: 0,
            },
            interpolation: Interpolation::Cubic,
            fade_in_total: 0,
            fade_in_remaining: 0,
            fade_out_total: 0,
            fade_out_remaining: 0,
            stop_after_fade: false,
            end_fade_frames: 0,
            pending: None,
            pending_order: 0,
            started_at: 0,
        }
    }

    /// Reset to a fully idle state. Safe to call from the audio thread.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// True while the voice contributes samples.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Current lifecycle phase.
    pub fn phase(&self) -> VoicePhase {
        if !self.active {
            VoicePhase::Idle
        } else if self.fade_out_remaining > 0 {
            VoicePhase::Releasing
        } else {
            VoicePhase::Playing
        }
    }

    /// Slot currently being played.
    pub fn sample_slot(&self) -> u16 {
        self.slot
    }

    /// Pad that triggered the voice.
    pub fn pad(&self) -> PadId {
        PadId::from_raw_unchecked(self.pad)
    }

    /// Channel count of the source buffer (1 or 2).
    pub fn source_channels(&self) -> u8 {
        self.source_channels
    }

    /// Playback mode captured at trigger time.
    pub fn mode(&self) -> PlayMode {
        self.mode
    }

    /// Current envelope estimate for the quietest-voice stealing policy.
    /// This is a cheap approximation, not an audio-rate level meter.
    pub fn level_estimate(&self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let fade_in = if self.fade_in_remaining > 0 {
            1.0 - self.fade_in_remaining as f32 / self.fade_in_total.max(1) as f32
        } else {
            1.0
        };
        let fade_out = if self.fade_out_remaining > 0 {
            self.fade_out_remaining as f32 / self.fade_out_total.max(1) as f32
        } else {
            1.0
        };
        self.velocity_gain * fade_in * fade_out
    }

    /// Drop a pending steal restart without touching the fade in progress.
    pub fn clear_pending(&mut self) {
        self.pending = None;
    }

    /// Playback position in native sample frames.
    pub fn position(&self) -> f64 {
        self.pos
    }

    /// Velocity gain derived from the trigger velocity.
    pub fn velocity_gain(&self) -> f32 {
        self.velocity_gain
    }

    /// Monotonic trigger order used by the oldest-voice stealing policy.
    pub fn started_at(&self) -> u64 {
        self.started_at
    }

    /// Frames remaining in the current fade-out, if any.
    pub fn fade_out_remaining(&self) -> u32 {
        self.fade_out_remaining
    }

    /// The spec this voice will restart with after a steal fade, if any.
    pub fn pending_spec(&self) -> Option<VoiceSpec> {
        self.pending
    }

    /// Start (or retrigger) the voice from a concrete sample buffer.
    ///
    /// Returns `false` when the buffer is unusable, leaving the voice idle.
    pub fn start(
        &mut self,
        sample: &SampleBuffer,
        spec: VoiceSpec,
        engine_rate: u32,
        order: u64,
    ) -> bool {
        self.reset();
        if sample.frame_count() == 0 || engine_rate == 0 {
            return false;
        }
        self.active = true;
        self.pad = spec.pad.get();
        self.slot = spec.sample_slot;
        self.frames = sample.frame_count().min(u32::MAX as usize) as u32;
        self.sample_rate = sample.sample_rate();
        self.source_channels = sample.channels();
        self.inc = self.sample_rate as f64 / engine_rate as f64
            * f64::powf(2.0, spec.tune_cents as f64 / 1200.0);
        if !self.inc.is_finite() || self.inc <= 0.0 {
            self.reset();
            return false;
        }
        self.velocity_gain = (spec.velocity.min(127) as f32 / 127.0).clamp(0.0, 1.0);
        self.mode = spec.playback.mode;
        self.loops = spec
            .playback
            .loop_points
            .sanitized(sample.frame_count(), self.mode);
        self.interpolation = spec.interpolation;
        self.fade_in_total = frames_for_secs(engine_rate, START_FADE_SECS).max(1);
        self.fade_in_remaining = self.fade_in_total;
        self.fade_out_total = 0;
        self.fade_out_remaining = 0;
        self.stop_after_fade = false;
        self.end_fade_frames = frames_for_secs(engine_rate, END_FADE_SECS).max(1);
        self.pending = None;
        self.pending_order = 0;
        self.started_at = order;
        true
    }

    /// Start from a bank lookup. Returns `false` if the slot is missing.
    pub fn start_from_bank(
        &mut self,
        bank: &SampleBank,
        spec: VoiceSpec,
        engine_rate: u32,
        order: u64,
    ) -> bool {
        match bank.get(spec.sample_slot) {
            Some(sample) => self.start(sample, spec, engine_rate, order),
            None => {
                self.reset();
                false
            }
        }
    }

    /// Begin a fade-out over `frames` samples (gate release, choke or steal).
    ///
    /// Calling this again while a fade is running keeps the shorter fade so a
    /// choke cannot be stretched out by a later release.
    pub fn begin_fade_out(&mut self, frames: u32) {
        if !self.active {
            return;
        }
        let frames = frames.max(1);
        if self.fade_out_remaining == 0 || frames < self.fade_out_remaining {
            self.fade_out_remaining = frames;
            self.fade_out_total = frames;
        }
        self.stop_after_fade = true;
    }

    /// Fade out and then restart with `spec` when the fade completes.
    ///
    /// This is the stealing path: the old voice fades click-free, then the
    /// stolen slot starts the new hit from frame zero.
    pub fn begin_steal(&mut self, frames: u32, spec: VoiceSpec, restart_order: u64) {
        self.begin_fade_out(frames);
        self.stop_after_fade = false;
        self.pending = Some(spec);
        self.pending_order = restart_order;
        // Claim the new order immediately. Until the fade finishes the voice
        // still renders the old sample, but it must no longer look like the
        // oldest voice to the next trigger in the same block.
        self.started_at = restart_order;
    }

    /// Render one stereo frame from `bank`.
    ///
    /// Returns `None` when the voice is idle. Samples are already scaled by
    /// velocity and the de-click/steal envelopes; pad gain and pan are applied
    /// by the mixer so they can be smoothed live.
    pub fn render(&mut self, bank: &SampleBank, engine_rate: u32) -> Option<(f32, f32)> {
        if !self.active {
            return None;
        }
        let sample = bank.get(self.slot)?;
        if sample.frame_count() as u32 != self.frames {
            self.reset();
            return None;
        }
        if self.mode != PlayMode::Loop && self.pos >= self.frames as f64 {
            self.reset();
            return None;
        }

        let mut left = self.interpolate(sample, self.pos, 0);
        let mut right = self.interpolate(sample, self.pos, 1);
        self.apply_loop_crossfade(sample, &mut left, &mut right);

        let mut gain = self.velocity_gain;

        // Fade-in envelope.
        if self.fade_in_remaining > 0 {
            let factor = 1.0 - self.fade_in_remaining as f32 / self.fade_in_total as f32;
            gain *= factor;
            self.fade_in_remaining -= 1;
        }

        // De-click the tail of a one-shot.
        if self.mode != PlayMode::Loop {
            let remaining = self.frames as f64 - self.pos;
            if remaining < self.end_fade_frames as f64 {
                let factor = (remaining / self.end_fade_frames as f64).clamp(0.0, 1.0) as f32;
                gain *= factor;
            }
        }

        // Fade-out envelope (release/choke/steal). The last sample of the
        // fade is exactly zero, so a stopped voice can never click.
        if self.fade_out_remaining > 0 {
            let factor =
                self.fade_out_remaining.saturating_sub(1) as f32 / self.fade_out_total as f32;
            gain *= factor;
            self.fade_out_remaining -= 1;
        }

        self.advance();

        if self.fade_out_remaining == 0 {
            if let Some(spec) = self.pending.take() {
                // Restart the slot with the pending hit and the fresh global
                // trigger order supplied by the engine.
                let order = self.pending_order;
                self.start_from_bank(bank, spec, engine_rate, order);
                return Some((0.0, 0.0));
            }
            if self.stop_after_fade {
                self.reset();
                return Some((0.0, 0.0));
            }
        }

        if self.mode != PlayMode::Loop && self.pos >= self.frames as f64 {
            self.active = false;
        }

        Some((left * gain, right * gain))
    }

    #[inline]
    fn advance(&mut self) {
        self.pos += self.inc;
        if self.mode == PlayMode::Loop {
            let ls = self.loops.start as f64;
            let le = self.loops.end as f64;
            let xf = self.loops.crossfade as f64;
            let body = (le - ls - xf).max(1.0);
            if self.pos >= le {
                let over = (self.pos - le) % body;
                self.pos = ls + xf + over;
            }
        }
    }

    fn apply_loop_crossfade(&self, sample: &SampleBuffer, left: &mut f32, right: &mut f32) {
        if self.mode != PlayMode::Loop || self.loops.crossfade == 0 {
            return;
        }
        let le = self.loops.end as f64;
        let xf = self.loops.crossfade as f64;
        let fade_start = le - xf;
        if self.pos < fade_start {
            return;
        }
        let t = ((self.pos - fade_start) / xf).clamp(0.0, 1.0) as f32;
        let head_pos = self.loops.start as f64 + (self.pos - fade_start);
        let tail_l = self.interpolate(sample, self.pos, 0);
        let tail_r = self.interpolate(sample, self.pos, 1);
        let head_l = self.interpolate(sample, head_pos, 0);
        let head_r = self.interpolate(sample, head_pos, 1);
        *left = tail_l * (1.0 - t) + head_l * t;
        *right = tail_r * (1.0 - t) + head_r * t;
    }

    #[inline]
    fn interpolate(&self, sample: &SampleBuffer, pos: f64, channel: usize) -> f32 {
        let base = pos.floor();
        let frac = (pos - base) as f32;
        let base = base as i64;
        match self.interpolation {
            Interpolation::Linear => {
                let s0 = self.read(sample, base, channel);
                let s1 = self.read(sample, base + 1, channel);
                s0 + (s1 - s0) * frac
            }
            Interpolation::Cubic => {
                let y0 = self.read(sample, base - 1, channel);
                let y1 = self.read(sample, base, channel);
                let y2 = self.read(sample, base + 1, channel);
                let y3 = self.read(sample, base + 2, channel);
                catmull_rom(y0, y1, y2, y3, frac)
            }
        }
    }

    #[inline]
    fn read(&self, sample: &SampleBuffer, mut frame: i64, channel: usize) -> f32 {
        let frames = self.frames as i64;
        if self.mode == PlayMode::Loop {
            let ls = self.loops.start as i64;
            let le = self.loops.end as i64;
            let len = (le - ls).max(1);
            if frame >= le {
                frame = ls + (frame - le).rem_euclid(len);
            } else if frame < ls && self.pos >= ls as f64 {
                frame = ls + (frame - ls).rem_euclid(len);
            }
        }
        let frame = frame.clamp(0, frames - 1) as usize;
        sample_value(sample, frame, channel)
    }
}

#[inline]
fn catmull_rom(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let a0 = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let a1 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let a2 = -0.5 * y0 + 0.5 * y2;
    ((a0 * t + a1) * t + a2) * t + y1
}

#[inline]
fn frames_for_secs(rate: u32, secs: f32) -> u32 {
    ((rate as f32 * secs).round() as u32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn ramp(frames: usize, channels: u8) -> SampleBuffer {
        let mut data = Vec::with_capacity(frames * channels as usize);
        for i in 0..frames {
            let v = i as f32 / frames as f32;
            for _ in 0..channels {
                data.push(v);
            }
        }
        SampleBuffer::new(data, channels, 48_000).unwrap()
    }

    fn bank_with(sample: SampleBuffer) -> SampleBank {
        let mut bank = SampleBank::new();
        bank.push(std::sync::Arc::new(sample));
        bank
    }

    #[test]
    fn rejects_empty_buffer() {
        assert_eq!(
            SampleBuffer::new(vec![], 2, 48_000),
            Err(SampleBufferError::Empty)
        );
    }

    #[test]
    fn rejects_bad_channels() {
        assert_eq!(
            SampleBuffer::new(vec![0.0; 4], 3, 48_000),
            Err(SampleBufferError::UnsupportedChannels(3))
        );
        assert_eq!(
            SampleBuffer::new(vec![0.0; 2], 0, 48_000),
            Err(SampleBufferError::UnsupportedChannels(0))
        );
    }

    #[test]
    fn rejects_bad_rate_and_misaligned_data() {
        assert_eq!(
            SampleBuffer::new(vec![0.0; 2], 2, 0),
            Err(SampleBufferError::InvalidSampleRate)
        );
        assert_eq!(
            SampleBuffer::new(vec![0.0; 3], 2, 48_000),
            Err(SampleBufferError::MisalignedData)
        );
    }

    #[test]
    fn mono_and_stereo_buffers_report_frames() {
        let mono = SampleBuffer::from_mono(vec![0.1, 0.2, 0.3], 44_100).unwrap();
        assert_eq!(mono.channels(), 1);
        assert_eq!(mono.frame_count(), 3);
        assert_eq!(mono.sample_rate(), 44_100);
        assert!((mono.duration_secs() - 3.0 / 44_100.0).abs() < 1e-12);

        let stereo = SampleBuffer::from_stereo(vec![0.1, -0.1, 0.2, -0.2], 48_000).unwrap();
        assert_eq!(stereo.channels(), 2);
        assert_eq!(stereo.frame_count(), 2);
        assert!((sample_value(&stereo, 1, 0) - 0.2).abs() < f32::EPSILON);
        assert!((sample_value(&stereo, 1, 1) + 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn one_shot_starts_and_finishes_without_staying_active() {
        let sample = ramp(100, 1);
        let bank = bank_with(sample.clone());
        let mut voice = Voice::new();
        assert!(voice.start(&sample, VoiceSpec::one_shot(0), 48_000, 1));
        assert!(voice.is_active());
        assert_eq!(voice.phase(), VoicePhase::Playing);

        let mut rendered = 0;
        for _ in 0..200 {
            if voice.render(&bank, 48_000).is_some() {
                rendered += 1;
            } else {
                break;
            }
        }
        assert_eq!(rendered, 100, "voice must stop exactly at buffer end");
        assert!(!voice.is_active());
        assert_eq!(voice.phase(), VoicePhase::Idle);
    }

    #[test]
    fn reset_returns_voice_to_idle() {
        let sample = ramp(64, 1);
        let bank = bank_with(sample.clone());
        let mut voice = Voice::new();
        voice.start(&sample, VoiceSpec::one_shot(0), 48_000, 7);
        assert!(voice.is_active());
        voice.begin_fade_out(16);
        assert_eq!(voice.phase(), VoicePhase::Releasing);
        voice.reset();
        assert!(!voice.is_active());
        assert_eq!(voice.phase(), VoicePhase::Idle);
        assert!(voice.render(&bank, 48_000).is_none());
    }

    #[test]
    fn release_fade_reaches_silence_and_stops() {
        let sample = SampleBuffer::from_mono(vec![1.0; 1000], 48_000).unwrap();
        let bank = bank_with(sample.clone());
        let mut voice = Voice::new();
        voice.start(&sample, VoiceSpec::one_shot(0), 48_000, 1);
        // Skip the start fade.
        for _ in 0..64 {
            voice.render(&bank, 48_000);
        }
        voice.begin_fade_out(48);
        assert_eq!(voice.phase(), VoicePhase::Releasing);
        let mut last = 1.0;
        for _ in 0..48 {
            let (l, _) = voice.render(&bank, 48_000).unwrap();
            assert!(l <= last);
            last = l;
        }
        assert!(!voice.is_active(), "gate release must free the voice");
    }

    #[test]
    fn gate_release_is_short_fade_not_hard_stop() {
        let sample = SampleBuffer::from_mono(vec![0.5; 480], 48_000).unwrap();
        let bank = bank_with(sample.clone());
        let mut voice = Voice::new();
        voice.start(
            &sample,
            VoiceSpec {
                playback: Playback {
                    mode: PlayMode::Gate,
                    loop_points: LoopPoints::default(),
                },
                ..VoiceSpec::one_shot(0)
            },
            48_000,
            1,
        );
        for _ in 0..100 {
            voice.render(&bank, 48_000);
        }
        voice.begin_fade_out(240); // 5 ms
        let mut samples = Vec::new();
        while let Some((l, _)) = voice.render(&bank, 48_000) {
            samples.push(l);
        }
        assert_eq!(samples.len(), 240);
        // The fade is monotonic and ends well below the body level.
        assert!(samples[0] < 0.5);
        assert!(samples[239].abs() < 0.01);
    }

    #[test]
    fn loop_mode_wraps_and_stays_active() {
        let sample = SampleBuffer::from_mono((0..480).map(|i| i as f32).collect(), 48_000).unwrap();
        let bank = bank_with(sample.clone());
        let mut voice = Voice::new();
        voice.start(
            &sample,
            VoiceSpec {
                playback: Playback {
                    mode: PlayMode::Loop,
                    loop_points: LoopPoints {
                        start: 100,
                        end: 300,
                        crossfade: 0,
                    },
                },
                ..VoiceSpec::one_shot(0)
            },
            48_000,
            1,
        );
        // Play past several loop iterations; must never go idle.
        for _ in 0..2000 {
            assert!(voice.render(&bank, 48_000).is_some());
        }
        assert!(voice.is_active());
        let pos = voice.position();
        assert!(
            (100.0..300.0).contains(&pos),
            "pos {pos} left the loop region"
        );
    }

    #[test]
    fn crossfaded_loop_has_no_discontinuity() {
        // A saw-ish buffer where a hard wrap would produce a large step.
        let frames = 512;
        let sample = SampleBuffer::from_mono(
            (0..frames)
                .map(|i| (i as f32 / frames as f32) * 2.0 - 1.0)
                .collect(),
            48_000,
        )
        .unwrap();
        let bank = bank_with(sample.clone());
        let mut voice = Voice::new();
        let xf = 64;
        voice.start(
            &sample,
            VoiceSpec {
                playback: Playback {
                    mode: PlayMode::Loop,
                    loop_points: LoopPoints {
                        start: 0,
                        end: frames as u32,
                        crossfade: xf,
                    },
                },
                ..VoiceSpec::one_shot(0)
            },
            48_000,
            1,
        );
        let mut last = 0.0;
        let mut max_step: f32 = 0.0;
        for _ in 0..4000 {
            let (l, _) = voice.render(&bank, 48_000).unwrap();
            max_step = max_step.max((l - last).abs());
            last = l;
        }
        // Without the crossfade the wrap would jump by nearly 2.0.
        assert!(max_step < 0.5, "loop produced a click, max step {max_step}");
    }

    #[test]
    fn cubic_interpolation_reconstructs_sine() {
        let rate = 48_000;
        let freq = 1000.0;
        let sample = SampleBuffer::from_mono(
            (0..4800)
                .map(|i| (2.0 * PI * freq * i as f32 / rate as f32).sin() * 0.5)
                .collect(),
            rate,
        )
        .unwrap();
        let bank = bank_with(sample.clone());
        let mut voice = Voice::new();
        voice.start(
            &sample,
            VoiceSpec {
                interpolation: Interpolation::Cubic,
                ..VoiceSpec::one_shot(0)
            },
            rate,
            1,
        );
        // Compare the rendered signal against the ideal sine; the start fade
        // is excluded.
        let mut max_err: f32 = 0.0;
        for i in 0..4000 {
            let (l, _) = voice.render(&bank, rate).unwrap();
            if i >= 64 {
                let ideal = (2.0 * PI * freq * i as f32 / rate as f32).sin() * 0.5;
                max_err = max_err.max((l - ideal).abs());
            }
        }
        assert!(max_err < 1e-3, "cubic interpolation error {max_err}");
    }

    #[test]
    fn pitch_shift_doubles_rate() {
        let rate = 48_000;
        let freq = 1000.0;
        let sample = SampleBuffer::from_mono(
            (0..48_000)
                .map(|i| (2.0 * PI * freq * i as f32 / rate as f32).sin() * 0.5)
                .collect(),
            rate,
        )
        .unwrap();
        let bank = bank_with(sample.clone());
        let mut voice = Voice::new();
        voice.start(
            &sample,
            VoiceSpec {
                tune_cents: 1200,
                ..VoiceSpec::one_shot(0)
            },
            rate,
            1,
        );
        // Count zero crossings over 0.5 seconds; expect 2 kHz.
        let mut crossings = 0;
        let mut prev = 0.0f32;
        for _ in 0..24_000 {
            if let Some((l, _)) = voice.render(&bank, rate) {
                if prev <= 0.0 && l > 0.0 {
                    crossings += 1;
                }
                prev = l;
            } else {
                break;
            }
        }
        // 2000 Hz over 0.5 s = 1000 cycles.
        assert!(
            (crossings - 1000i32).abs() <= 2,
            "expected ~1000 cycles, got {crossings}"
        );
    }

    #[test]
    fn pitch_accuracy_is_within_one_cent() {
        let rate = 48_000u32;
        let base_freq = 440.0f64;
        let cents = 700.0f64;
        let ratio = 2f64.powf(cents / 1200.0);
        let expected_freq = base_freq * ratio;
        // Eight seconds of source lasts ~5.3 s when pitched up 700 cents.
        let source_frames = (rate as f64 * 8.0) as usize;
        let sample = SampleBuffer::from_mono(
            (0..source_frames)
                .map(|i| {
                    (2.0 * std::f64::consts::PI * base_freq * i as f64 / rate as f64).sin() as f32
                        * 0.5
                })
                .collect(),
            rate,
        )
        .unwrap();
        let bank = bank_with(sample.clone());
        let mut voice = Voice::new();
        voice.start(
            &sample,
            VoiceSpec {
                tune_cents: cents as i32,
                ..VoiceSpec::one_shot(0)
            },
            rate,
            1,
        );
        // Skip the start fade before counting crossings.
        for _ in 0..1000 {
            voice.render(&bank, rate);
        }
        let frames = rate as usize * 5;
        let mut crossings = 0i64;
        let mut rendered = 0usize;
        let mut prev = 0.0f32;
        for _ in 0..frames {
            let Some((left, _)) = voice.render(&bank, rate) else {
                break;
            };
            if prev <= 0.0 && left > 0.0 {
                crossings += 1;
            }
            prev = left;
            rendered += 1;
        }
        let measured = crossings as f64 / (rendered as f64 / rate as f64);
        let cents_error = 1200.0 * (measured / expected_freq).log2().abs();
        assert!(
            cents_error < 1.0,
            "pitch error {cents_error:.3} cents (expected {expected_freq:.3} Hz, measured {measured:.3} Hz)"
        );
    }

    #[test]
    fn one_shot_start_and_end_are_de_clicked() {
        let sample = SampleBuffer::from_mono(vec![0.5; 480], 48_000).unwrap();
        let bank = bank_with(sample.clone());
        let mut voice = Voice::new();
        voice.start(&sample, VoiceSpec::one_shot(0), 48_000, 1);
        let mut last = 0.0f32;
        let mut max_step = 0.0f32;
        while let Some((left, _)) = voice.render(&bank, 48_000) {
            max_step = max_step.max((left - last).abs());
            last = left;
        }
        // A hard start would step by 0.5; the de-click fades spread it over
        // 24 frames, so no single step exceeds ~0.03.
        assert!(
            max_step < 0.05,
            "one-shot start/end clicked with a {max_step} step"
        );
        assert!(!voice.is_active());
    }

    #[test]
    fn stealing_fades_then_restarts_with_new_spec() {
        let sample_a = SampleBuffer::from_mono(vec![0.8; 4800], 48_000).unwrap();
        let sample_b = SampleBuffer::from_mono(vec![0.2; 4800], 48_000).unwrap();
        let mut bank = SampleBank::new();
        bank.push(std::sync::Arc::new(sample_a.clone()));
        bank.push(std::sync::Arc::new(sample_b.clone()));

        let mut voice = Voice::new();
        voice.start(&sample_a, VoiceSpec::one_shot(0), 48_000, 1);
        for _ in 0..100 {
            voice.render(&bank, 48_000);
        }
        voice.begin_steal(96, VoiceSpec::one_shot(1), 2); // 2 ms
        assert_eq!(voice.pending_spec().unwrap().sample_slot, 1);

        let mut saw_zero = false;
        for _ in 0..200 {
            if let Some((l, _)) = voice.render(&bank, 48_000) {
                if l.abs() < 1e-6 {
                    saw_zero = true;
                }
            }
        }
        assert!(saw_zero, "steal should fade through zero");
        assert_eq!(voice.sample_slot(), 1, "voice must restart with new sample");
    }
}
