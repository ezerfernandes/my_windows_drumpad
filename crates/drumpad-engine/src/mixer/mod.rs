//! The real-time mixer: pad runtime parameters, the voice pool, sample-bank
//! retirement, metering, and the per-sample render loop.
//!
//! [`Engine::process`] is the real-time entry point. It never allocates,
//! locks, logs, or touches the filesystem. All heap use happens in
//! [`Engine::new`] and in non-real-time setup calls.

use crate::limiter::{Limiter, LimiterConfig};
use crate::meter::{Meter, MeterReader, MeterShared};
use crate::queue::{Command, QueueConsumer, QueueProducer, StealPolicy};
use crate::voice::{Interpolation, Playback, SampleBank, Voice, VoiceSpec};
use drumpad_input::trigger::{
    EventConsumer, EventProducer, InputEvent, PushError, TriggerSource, event_queue,
};
use drumpad_model::ids::{PadId, Velocity};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Number of performance pads.
pub const PAD_COUNT: usize = drumpad_model::ids::PAD_COUNT;

/// Default input-event queue capacity (power of two, required by the input
/// crate's SPSC queue).
pub const DEFAULT_TRIGGER_CAPACITY: usize = 4096;

/// Allocated voice ceiling.
pub const MAX_VOICES: usize = 256;

/// Lowest selectable voice limit.
pub const MIN_VOICES: usize = 16;

/// Default voice limit from the design document.
pub const DEFAULT_VOICES: usize = 64;

/// Maximum number of trigger events moved from the queue into the scheduler per
/// block. Keeps the callback bounded even under a pathological event storm.
const PENDING_MAX: usize = 64;

/// Maximum commands applied per block.
const COMMAND_BUDGET: usize = 256;

/// Maximum number of retired sample banks the engine can hand back before it
/// has to defer a swap. Each entry is only an `Arc`, so the memory cost is
/// negligible.
const RETIRE_QUEUE_CAPACITY: usize = 8;

/// Fixed overflow slots that hold retired banks while the return queue is
/// full. Together with the queue this bounds how many uncollected banks the
/// audio thread can hold; a swap is deferred rather than dropping an `Arc` in
/// the callback.
const RETIRE_OVERFLOW_SLOTS: usize = 16;

/// Crossfade used when a new hit steals a voice, in milliseconds.
pub const STEAL_FADE_MS: f32 = 2.0;
/// Release fade for gate voices, in milliseconds.
pub const RELEASE_FADE_MS: f32 = 5.0;
/// Choke fade, in milliseconds.
pub const CHOKE_FADE_MS: f32 = 6.0;
/// Panic fade, in milliseconds.
pub const PANIC_FADE_MS: f32 = 5.0;
/// Fade used while a kit's sample bank is swapped, in milliseconds.
pub const BANK_SWAP_FADE_MS: f32 = 5.0;
/// Gain and pan smoothing time, in milliseconds.
pub const PARAM_SMOOTH_MS: f32 = 10.0;

/// Engine configuration. All fields have musically sensible defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    /// Device sample rate. Voices resample their native buffer to this rate.
    pub sample_rate: u32,
    /// Number of voices allocated up front, clamped to `16..=256`.
    pub max_voices: usize,
    /// Voices the allocator may use at startup, clamped to `16..=max_voices`.
    pub voice_limit: usize,
    /// Interpolation used for newly triggered voices.
    pub interpolation: Interpolation,
    /// Voice-stealing policy.
    pub steal_policy: StealPolicy,
    /// Master limiter configuration.
    pub limiter: LimiterConfig,
    /// Trigger queue capacity. Must be at least 16.
    pub trigger_capacity: usize,
    /// Command queue capacity. Must be at least 16.
    pub command_capacity: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            max_voices: MAX_VOICES,
            voice_limit: DEFAULT_VOICES,
            interpolation: Interpolation::Cubic,
            steal_policy: StealPolicy::Oldest,
            limiter: LimiterConfig::default(),
            trigger_capacity: DEFAULT_TRIGGER_CAPACITY,
            command_capacity: crate::queue::DEFAULT_COMMAND_CAPACITY,
        }
    }
}

/// Pad settings mirrored in the engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PadSettings {
    /// Sample bank slot, or `None` for a silent pad.
    pub sample_slot: Option<u16>,
    /// Linear gain, `0.0..=2.0`.
    pub gain: f32,
    /// Pan, `-1.0` left .. `1.0` right.
    pub pan: f32,
    /// Tuning in cents, applied on the next trigger.
    pub tune_cents: i32,
    /// Choke group, `0` disables choking, `1..=16` is a group id.
    pub choke_group: u8,
    /// Playback mode and loop points, applied on the next trigger.
    pub playback: Playback,
}

impl Default for PadSettings {
    fn default() -> Self {
        Self {
            sample_slot: None,
            gain: 1.0,
            pan: 0.0,
            tune_cents: 0,
            choke_group: 0,
            playback: Playback::default(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PadRuntime {
    settings: PadSettings,
    gain_current: f32,
    gain_target: f32,
    pan_current: f32,
    pan_target: f32,
    pan_left: f32,
    pan_right: f32,
    balance_left: f32,
    balance_right: f32,
}

impl PadRuntime {
    fn new() -> Self {
        let mut pad = Self {
            settings: PadSettings::default(),
            gain_current: 1.0,
            gain_target: 1.0,
            pan_current: 0.0,
            pan_target: 0.0,
            pan_left: std::f32::consts::FRAC_1_SQRT_2,
            pan_right: std::f32::consts::FRAC_1_SQRT_2,
            balance_left: 1.0,
            balance_right: 1.0,
        };
        pad.recompute();
        pad
    }

    #[inline]
    fn advance(&mut self, gain_coeff: f32, pan_coeff: f32) {
        self.gain_current += (self.gain_target - self.gain_current) * gain_coeff;
        self.pan_current += (self.pan_target - self.pan_current) * pan_coeff;
        self.recompute();
    }

    #[inline]
    fn recompute(&mut self) {
        let pan = self.pan_current.clamp(-1.0, 1.0);
        // Equal-power law for mono sources: centre is -3 dB per channel.
        let theta = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        self.pan_left = theta.cos();
        self.pan_right = theta.sin();
        // Balance law for stereo sources: centre is unity on both channels.
        self.balance_left = if pan <= 0.0 { 1.0 } else { 1.0 - pan };
        self.balance_right = if pan >= 0.0 { 1.0 } else { 1.0 + pan };
    }

    fn set_gain(&mut self, gain: f32) {
        let gain = gain.clamp(0.0, 2.0);
        self.settings.gain = gain;
        self.gain_target = gain;
    }

    fn set_pan(&mut self, pan: f32) {
        let pan = pan.clamp(-1.0, 1.0);
        self.settings.pan = pan;
        self.pan_target = pan;
    }
}

/// A read-only snapshot of one voice, for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceSnapshot {
    /// Whether the voice is contributing samples.
    pub active: bool,
    /// Pad that owns the voice.
    pub pad: PadId,
    /// Sample bank slot.
    pub sample_slot: u16,
    /// Playback mode captured at trigger time.
    pub mode: crate::voice::PlayMode,
    /// Trigger order.
    pub started_at: u64,
    /// Playback position in native sample frames.
    pub position: f64,
}

/// The UI/input side of the engine: produces triggers and commands, consumes
/// retired sample banks and meter snapshots.
pub struct EngineHandle {
    /// Canonical input-event producer. Share it with the keyboard/MIDI capture
    /// threads or forward events through [`EngineHandle::send_input`].
    pub events: EventProducer,
    /// Command queue producer.
    pub commands: QueueProducer<Command>,
    /// Retired sample banks. Drain and drop these on a non-real-time thread.
    pub retired_banks: QueueConsumer<Arc<SampleBank>>,
    meter: MeterReader,
    sample_rate: u32,
    current_frame: Arc<AtomicU64>,
    dropped_commands: Arc<AtomicU64>,
}

impl EngineHandle {
    /// The engine playhead at the start of the next block, in frames.
    pub fn timestamp_now(&self) -> u64 {
        self.current_frame.load(Ordering::Relaxed)
    }

    /// The engine playhead in microseconds, for stamping input events.
    pub fn timestamp_now_us(&self) -> u64 {
        self.timestamp_now().saturating_mul(1_000_000) / u64::from(self.sample_rate.max(1))
    }

    /// Forward a canonical input event (the App's normal path).
    ///
    /// The producer is single-owner: the app's input aggregator (keyboard,
    /// MIDI, UI preview) must funnel events through this one handle. Events
    /// whose `timestamp_us` comes from a different clock origin are clamped to
    /// "now" when they are more than a second ahead of the engine playhead.
    pub fn send_input(&self, event: InputEvent) -> Result<(), PushError> {
        self.events.push(event)
    }

    /// Queue a note-on for `pad` at the current playhead.
    pub fn note_on(
        &self,
        pad: PadId,
        velocity: Velocity,
        source: TriggerSource,
    ) -> Result<(), PushError> {
        let timestamp_us = self.timestamp_now_us();
        let event = drumpad_input::trigger::TriggerEvent::new(
            pad,
            Velocity::clamped(velocity.get()),
            source,
            timestamp_us,
        );
        self.events.push(event)
    }

    /// Queue a note-off for `pad` at the current playhead.
    pub fn note_off(&self, pad: PadId, source: TriggerSource) -> Result<(), PushError> {
        let timestamp_us = self.timestamp_now_us();
        let event = drumpad_input::trigger::ReleaseEvent::new(pad, source, timestamp_us);
        self.events.push(event)
    }

    /// Release every gated voice.
    pub fn all_notes_off(&mut self) -> Result<(), Command> {
        self.commands.push(Command::AllNotesOff)
    }

    /// Stop every voice with a short fade.
    pub fn panic(&mut self) -> Result<(), Command> {
        self.commands.push(Command::Panic)
    }

    /// Send a non-real-time command.
    pub fn send(&mut self, command: Command) -> Result<(), Command> {
        self.commands.push(command)
    }

    /// Read the latest meters.
    pub fn meter(&self) -> &MeterReader {
        &self.meter
    }

    /// Number of triggers dropped because the input queue was full.
    pub fn dropped_triggers(&self) -> u64 {
        self.events.dropped()
    }

    /// Number of commands dropped because the queue was full.
    pub fn dropped_commands(&self) -> u64 {
        self.dropped_commands.load(Ordering::Relaxed)
    }

    /// Drain retired banks and drop them on the calling (non-real-time)
    /// thread. Returns how many were collected.
    pub fn collect_retired_banks(&mut self) -> usize {
        let mut count = 0;
        while self.retired_banks.pop().is_some() {
            count += 1;
        }
        count
    }

    /// Retired banks waiting to be collected.
    pub fn retired_bank_count(&self) -> usize {
        self.retired_banks.len()
    }
}

/// The real-time engine. Create it with [`Engine::new`], which also returns the
/// handle used by the UI and input threads.
pub struct Engine {
    config: EngineConfig,
    sample_rate: u32,
    voices: Vec<Voice>,
    voice_limit: usize,
    next_order: u64,
    pads: [PadRuntime; PAD_COUNT],
    bank: Option<Arc<SampleBank>>,
    pending_bank: Option<Arc<SampleBank>>,
    retire_tx: QueueProducer<Arc<SampleBank>>,
    retired_overflow: [Option<Arc<SampleBank>>; RETIRE_OVERFLOW_SLOTS],
    events: EventConsumer,
    command_rx: QueueConsumer<Command>,
    pending: [Option<(u64, InputEvent)>; PENDING_MAX],
    pending_len: usize,
    limiter: Limiter,
    interpolation: Interpolation,
    steal_policy: StealPolicy,
    meter: Meter,
    meter_shared: Arc<MeterShared>,
    playhead: u64,
    playhead_atomic: Arc<AtomicU64>,
    gain_coeff: f32,
    pan_coeff: f32,
    release_frames: u32,
    choke_frames: u32,
    steal_frames: u32,
    panic_frames: u32,
    swap_frames: u32,
}

impl Engine {
    /// Create the engine and its handle. All allocation happens here.
    pub fn new(config: EngineConfig) -> (Self, EngineHandle) {
        let sample_rate = config.sample_rate.max(8_000);
        let max_voices = config.max_voices.clamp(MIN_VOICES, MAX_VOICES);
        let voice_limit = config.voice_limit.clamp(MIN_VOICES, max_voices);
        let voices = (0..max_voices).map(|_| Voice::new()).collect();

        let (event_tx, event_rx) = event_queue(config.trigger_capacity.max(64).next_power_of_two());
        let (command_tx, command_rx, dropped_commands) =
            command_queue_with_capacity(config.command_capacity);
        let (retire_tx, retire_rx, _) = {
            let (p, c, d) = crate::queue::queue(RETIRE_QUEUE_CAPACITY);
            (p, c, d)
        };

        let meter_shared = Arc::new(MeterShared::new());
        let playhead_atomic = Arc::new(AtomicU64::new(0));

        let meter = MeterReader::new(Arc::clone(&meter_shared));

        let engine = Self {
            config: config.clone(),
            sample_rate,
            voices,
            voice_limit,
            next_order: 0,
            pads: [PadRuntime::new(); PAD_COUNT],
            bank: None,
            pending_bank: None,
            retire_tx,
            retired_overflow: [const { None }; RETIRE_OVERFLOW_SLOTS],
            events: event_rx,
            command_rx,
            pending: [None; PENDING_MAX],
            pending_len: 0,
            limiter: Limiter::new(sample_rate, config.limiter),
            interpolation: config.interpolation,
            steal_policy: config.steal_policy,
            meter: Meter::new(),
            meter_shared: Arc::clone(&meter_shared),
            playhead: 0,
            playhead_atomic: Arc::clone(&playhead_atomic),
            gain_coeff: smoothing_coeff(sample_rate, PARAM_SMOOTH_MS),
            pan_coeff: smoothing_coeff(sample_rate, PARAM_SMOOTH_MS),
            release_frames: ms_to_frames(sample_rate, RELEASE_FADE_MS),
            choke_frames: ms_to_frames(sample_rate, CHOKE_FADE_MS),
            steal_frames: ms_to_frames(sample_rate, STEAL_FADE_MS),
            panic_frames: ms_to_frames(sample_rate, PANIC_FADE_MS),
            swap_frames: ms_to_frames(sample_rate, BANK_SWAP_FADE_MS),
        };

        let handle = EngineHandle {
            events: event_tx,
            commands: command_tx,
            retired_banks: retire_rx,
            meter,
            sample_rate,
            current_frame: playhead_atomic,
            dropped_commands,
        };

        (engine, handle)
    }

    /// Device sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Current voice limit.
    pub fn voice_limit(&self) -> usize {
        self.voice_limit
    }

    /// Number of active voices.
    pub fn active_voice_count(&self) -> usize {
        self.voices.iter().filter(|v| v.is_active()).count()
    }

    /// Current engine configuration.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Pad settings as currently mirrored in the engine.
    pub fn pad_settings(&self, pad: PadId) -> Option<PadSettings> {
        self.pads.get(pad.index()).map(|p| p.settings)
    }

    /// Snapshot of one voice for diagnostics.
    pub fn voice_snapshot(&self, index: usize) -> Option<VoiceSnapshot> {
        self.voices.get(index).map(|v| VoiceSnapshot {
            active: v.is_active(),
            pad: v.pad(),
            sample_slot: v.sample_slot(),
            mode: v.mode(),
            started_at: v.started_at(),
            position: v.position(),
        })
    }

    /// Shared meter cells, for wiring into the stream diagnostics.
    pub fn meter_shared(&self) -> Arc<MeterShared> {
        Arc::clone(&self.meter_shared)
    }

    /// Render `frames` interleaved stereo frames into `out`.
    ///
    /// `out` must contain at least `frames * 2` samples. Extra samples are left
    /// untouched. This is the only method that must run on the audio thread.
    pub fn process(&mut self, out: &mut [f32], frames: usize) {
        let frames = frames.min(out.len() / 2);
        if frames == 0 {
            return;
        }

        self.apply_commands();
        self.drain_triggers();

        let block_start = self.playhead;
        let block_end = block_start.saturating_add(frames as u64);

        // Move input events whose frame timestamp falls inside this block into
        // a small fixed due-list. Stable compaction keeps FIFO order for equal
        // timestamps, which makes voice allocation deterministic.
        let mut due = [(0u32, EMPTY_INPUT); PENDING_MAX];
        let mut due_len = 0usize;
        let mut write = 0usize;
        for read in 0..self.pending_len {
            let Some((event_frame, event)) = self.pending[read] else {
                continue;
            };
            if event_frame < block_end {
                let offset = event_frame
                    .saturating_sub(block_start)
                    .min(frames as u64 - 1) as u32;
                due[due_len] = (offset, event);
                due_len += 1;
            } else {
                self.pending[write] = Some((event_frame, event));
                write += 1;
            }
        }
        for slot in self.pending.iter_mut().take(self.pending_len).skip(write) {
            *slot = None;
        }
        self.pending_len = write;
        // Insertion sort by offset. Equal offsets keep their FIFO order.
        for a in 1..due_len {
            let mut j = a;
            while j > 0 && due[j - 1].0 > due[j].0 {
                due.swap(j - 1, j);
                j -= 1;
            }
        }

        self.meter.begin_block();
        let mut due_index = 0usize;
        for frame in 0..frames {
            while due_index < due_len && due[due_index].0 as usize <= frame {
                self.apply_event(due[due_index].1);
                due_index += 1;
            }
            self.render_frame(out, frame);
        }

        self.playhead = block_end;
        self.playhead_atomic.store(self.playhead, Ordering::Relaxed);

        let values = self.meter.finish_block();
        self.meter_shared.store(values);
        self.meter_shared
            .store_voice_counts(self.active_voice_count(), self.voice_limit);

        self.try_apply_pending_bank();
    }

    /// Render one stereo frame. Pad gain and pan are smoothed per sample.
    #[inline]
    fn render_frame(&mut self, out: &mut [f32], frame: usize) {
        for pad in &mut self.pads {
            pad.advance(self.gain_coeff, self.pan_coeff);
        }

        let mut left = 0.0f32;
        let mut right = 0.0f32;
        if let Some(bank) = self.bank.as_ref() {
            for voice in self.voices.iter_mut() {
                if let Some((vl, vr)) = voice.render(bank, self.sample_rate) {
                    let pad = &self.pads[voice.pad().index().min(PAD_COUNT - 1)];
                    let gain = pad.gain_current;
                    if voice.source_channels() == 1 {
                        left += vl * gain * pad.pan_left;
                        right += vr * gain * pad.pan_right;
                    } else {
                        left += vl * gain * pad.balance_left;
                        right += vr * gain * pad.balance_right;
                    }
                }
            }
        }

        let (left, right) = self.limiter.process(left, right);
        out[frame * 2] = left;
        out[frame * 2 + 1] = right;
        self.meter.accumulate(left, right);
    }

    fn apply_commands(&mut self) {
        for _ in 0..COMMAND_BUDGET {
            // Never take ownership of a sample bank unless we can retire the
            // displaced one without dropping an `Arc` on the audio thread.
            let take = match self.command_rx.peek() {
                Some(Command::SwapSampleBank(_)) => {
                    self.pending_bank.is_none() || self.retire_capacity_available()
                }
                _ => true,
            };
            if !take {
                break;
            }
            let Some(command) = self.command_rx.pop() else {
                break;
            };
            match command {
                Command::SetPadGain { pad, gain } => {
                    if let Some(p) = self.pads.get_mut(pad.index()) {
                        p.set_gain(gain);
                    }
                }
                Command::SetPadPan { pad, pan } => {
                    if let Some(p) = self.pads.get_mut(pad.index()) {
                        p.set_pan(pan);
                    }
                }
                Command::SetPadTune { pad, cents } => {
                    if let Some(p) = self.pads.get_mut(pad.index()) {
                        p.settings.tune_cents = cents.clamp(-4800, 4800);
                    }
                }
                Command::SetPadChokeGroup { pad, group } => {
                    if let Some(p) = self.pads.get_mut(pad.index()) {
                        p.settings.choke_group = group.min(16);
                    }
                }
                Command::SetPadPlayback { pad, playback } => {
                    if let Some(p) = self.pads.get_mut(pad.index()) {
                        p.settings.playback = playback;
                    }
                }
                Command::SetPadSample { pad, slot } => {
                    if let Some(p) = self.pads.get_mut(pad.index()) {
                        p.settings.sample_slot = Some(slot);
                    }
                }
                Command::SetVoiceLimit { voices } => {
                    let limit = (voices as usize).clamp(MIN_VOICES, self.voices.len());
                    if limit < self.voice_limit {
                        let fade = self.panic_frames;
                        for voice in self.voices.iter_mut().skip(limit) {
                            voice.begin_fade_out(fade);
                            voice.clear_pending();
                        }
                    }
                    self.voice_limit = limit;
                }
                Command::SetStealPolicy(policy) => {
                    self.steal_policy = policy;
                }
                Command::SetInterpolation(interpolation) => {
                    self.interpolation = interpolation;
                }
                Command::SetLimiterEnabled(enabled) => {
                    self.limiter.set_enabled(enabled);
                }
                Command::SwapSampleBank(bank) => {
                    // The precheck above guarantees a retire slot exists for a
                    // displaced pending bank.
                    if let Some(previous) = self.pending_bank.replace(bank) {
                        self.retire_bank(previous);
                    }
                    self.fade_all(self.swap_frames, true);
                }
                Command::AllNotesOff => {
                    self.release_all_gated();
                }
                Command::Panic => {
                    self.panic();
                }
                Command::ResetMeters => {
                    self.meter_shared.reset();
                }
            }
        }
    }

    fn drain_triggers(&mut self) {
        while self.pending_len < PENDING_MAX {
            let Some(event) = self.events.pop() else {
                break;
            };
            let frame = self.event_frame(event.timestamp_us());
            self.pending[self.pending_len] = Some((frame, event));
            self.pending_len += 1;
        }
    }

    /// Convert an input timestamp (microseconds, monotonic capture clock) to an
    /// engine frame. An event more than a second in the future is treated as
    /// "now"; that keeps a mismatched clock origin from parking a hit forever.
    fn event_frame(&self, timestamp_us: u64) -> u64 {
        let frame = timestamp_us.saturating_mul(u64::from(self.sample_rate)) / 1_000_000;
        let horizon = self
            .playhead
            .saturating_add(u64::from(self.sample_rate.max(1)));
        if frame > horizon {
            self.playhead
        } else {
            frame
        }
    }

    fn apply_event(&mut self, event: InputEvent) {
        match event {
            InputEvent::Trigger(trigger) => {
                if self.pending_bank.is_none() {
                    self.trigger_pad(trigger.pad.index(), trigger.velocity.get());
                }
            }
            InputEvent::Release(release) => {
                self.release_pad(release.pad.index());
            }
        }
    }

    fn trigger_pad(&mut self, pad_index: usize, velocity: u8) {
        if pad_index >= PAD_COUNT {
            return;
        }
        let Some(bank) = self.bank.as_ref() else {
            return;
        };
        let settings = self.pads[pad_index].settings;
        let Some(slot) = settings.sample_slot else {
            return;
        };
        if bank.get(slot).is_none() {
            return;
        }

        // Choke every voice whose pad shares this pad's choke group. This
        // includes earlier hits of the same pad (closed hi-hat retrigger).
        if settings.choke_group != 0 {
            let group = settings.choke_group;
            for (index, choked) in self.pads.iter().enumerate() {
                if choked.settings.choke_group != group {
                    continue;
                }
                let fade = self.choke_frames;
                for voice in self.voices.iter_mut() {
                    if voice.is_active() && voice.pad().index() == index {
                        voice.begin_fade_out(fade);
                        voice.clear_pending();
                    }
                }
            }
        }

        let spec = VoiceSpec {
            pad: PadId::from_raw_unchecked(pad_index.min(PAD_COUNT - 1) as u8),
            sample_slot: slot,
            playback: settings.playback,
            tune_cents: settings.tune_cents,
            velocity: velocity.min(127),
            interpolation: self.interpolation,
        };

        let mut idle = None;
        for (index, voice) in self.voices.iter().enumerate().take(self.voice_limit) {
            if !voice.is_active() {
                idle = Some(index);
                break;
            }
        }

        if let Some(index) = idle {
            if self.voices[index].start_from_bank(bank, spec, self.sample_rate, self.next_order) {
                self.next_order = self.next_order.wrapping_add(1);
            }
            return;
        }

        // Pool exhausted: steal deterministically and fade the victim. The
        // restart receives the new global order so the oldest policy advances
        // instead of re-stealing the same slot forever.
        let victim = self.pick_victim();
        self.voices[victim].begin_steal(self.steal_frames, spec, self.next_order);
        self.next_order = self.next_order.wrapping_add(1);
    }

    fn pick_victim(&self) -> usize {
        let limit = self.voice_limit.min(self.voices.len());
        debug_assert!(limit > 0);
        match self.steal_policy {
            StealPolicy::Oldest => {
                let mut best = 0usize;
                let mut best_key = u64::MAX;
                for (index, voice) in self.voices.iter().enumerate().take(limit) {
                    if !voice.is_active() {
                        return index;
                    }
                    if voice.started_at() < best_key {
                        best_key = voice.started_at();
                        best = index;
                    }
                }
                best
            }
            StealPolicy::Quietest => {
                let mut best = 0usize;
                let mut best_key = f32::INFINITY;
                for (index, voice) in self.voices.iter().enumerate().take(limit) {
                    if !voice.is_active() {
                        return index;
                    }
                    let key = voice.level_estimate();
                    if key < best_key {
                        best_key = key;
                        best = index;
                    }
                }
                best
            }
        }
    }

    fn release_pad(&mut self, pad: usize) {
        let fade = self.release_frames;
        for voice in self.voices.iter_mut() {
            if voice.is_active()
                && voice.pad().index() == pad
                && voice.mode() == crate::voice::PlayMode::Gate
            {
                voice.begin_fade_out(fade);
            }
        }
    }

    fn release_all_gated(&mut self) {
        let fade = self.release_frames;
        for voice in self.voices.iter_mut() {
            if voice.is_active() && voice.mode() == crate::voice::PlayMode::Gate {
                voice.begin_fade_out(fade);
            }
        }
    }

    fn panic(&mut self) {
        self.fade_all(self.panic_frames, true);
        self.pending_len = 0;
        while self.events.pop().is_some() {}
    }

    fn fade_all(&mut self, frames: u32, clear_pending: bool) {
        for voice in self.voices.iter_mut() {
            if voice.is_active() {
                voice.begin_fade_out(frames);
                if clear_pending {
                    voice.clear_pending();
                }
            }
        }
    }

    fn try_apply_pending_bank(&mut self) {
        if self.pending_bank.is_none() || self.active_voice_count() > 0 {
            return;
        }
        if !self.retire_capacity_available() {
            // Keep the pending bank and retry next block rather than dropping
            // the installed bank on the audio thread.
            return;
        }
        let Some(pending) = self.pending_bank.take() else {
            return;
        };
        if let Some(old) = self.bank.take() {
            self.retire_bank(old);
        }
        self.bank = Some(pending);
    }

    fn retire_capacity_available(&self) -> bool {
        self.retire_tx.len() < self.retire_tx.capacity()
            || self.retired_overflow.iter().any(|slot| slot.is_none())
    }

    /// Move a bank towards the UI thread. Callers precheck
    /// [`Engine::retire_capacity_available`], so a slot always exists; the
    /// `mem::forget` fallback guarantees that a bug can never free memory on
    /// the audio thread (it leaks instead).
    fn retire_bank(&mut self, bank: Arc<SampleBank>) {
        self.flush_retired();
        if let Err(bank) = self.retire_tx.push(bank) {
            if let Some(free) = self.retired_overflow.iter().position(|slot| slot.is_none()) {
                self.retired_overflow[free] = Some(bank);
            } else {
                debug_assert!(false, "retire overflow exhausted despite precheck");
                std::mem::forget(bank);
            }
        }
    }

    fn flush_retired(&mut self) {
        for slot in self.retired_overflow.iter_mut() {
            if let Some(held) = slot.take() {
                match self.retire_tx.push(held) {
                    Ok(()) => {}
                    Err(held) => {
                        *slot = Some(held);
                    }
                }
            }
        }
    }
}

/// Contract name from `docs/architecture/interfaces.md`.
pub type PlaybackEngine = Engine;
/// Contract name for the parameter command type.
pub type EngineCommand = Command;

/// A point-in-time engine snapshot for diagnostics and the UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineSnapshot {
    /// Peak/RMS meters for the last block.
    pub meter: crate::meter::MeterValues,
    /// Voices currently producing samples.
    pub active_voices: usize,
    /// Configured voice limit.
    pub voice_limit: usize,
    /// Next engine frame.
    pub playhead: u64,
}

impl Engine {
    /// Snapshot meters, voice count, and playhead. Safe from the UI thread.
    pub fn snapshot(&self) -> EngineSnapshot {
        EngineSnapshot {
            meter: self.meter_shared.load(),
            active_voices: self.active_voice_count(),
            voice_limit: self.voice_limit,
            playhead: self.playhead,
        }
    }
}

const EMPTY_INPUT: InputEvent = InputEvent::Release(drumpad_input::trigger::ReleaseEvent::new(
    PadId::FIRST,
    TriggerSource::Keyboard,
    0,
));

fn command_queue_with_capacity(
    capacity: usize,
) -> (
    QueueProducer<Command>,
    QueueConsumer<Command>,
    Arc<AtomicU64>,
) {
    crate::queue::queue(capacity.max(16))
}

#[inline]
fn ms_to_frames(rate: u32, ms: f32) -> u32 {
    ((rate as f32 * ms / 1000.0).round() as u32).max(1)
}

#[inline]
fn smoothing_coeff(rate: u32, ms: f32) -> f32 {
    let frames = ms_to_frames(rate, ms) as f32;
    (1.0 - (-1.0 / frames).exp()).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::{LoopPoints, PlayMode, SampleBuffer};
    use std::f32::consts::FRAC_1_SQRT_2;

    const RATE: u32 = 48_000;

    fn pad(index: u8) -> PadId {
        PadId::from_raw_unchecked(index)
    }

    fn all_pads() -> impl Iterator<Item = PadId> {
        (0..PAD_COUNT).map(|index| PadId::from_raw_unchecked(index as u8))
    }

    fn velocity(value: u8) -> Velocity {
        Velocity::new(value).unwrap()
    }

    fn engine(voices: usize) -> (Engine, EngineHandle) {
        let config = EngineConfig {
            voice_limit: voices,
            max_voices: MAX_VOICES,
            ..EngineConfig::default()
        };
        Engine::new(config)
    }

    fn constant_sample(frames: usize, value: f32) -> SampleBuffer {
        SampleBuffer::from_mono(vec![value; frames], RATE).unwrap()
    }

    fn install(engine: &mut Engine, handle: &mut EngineHandle, sample: SampleBuffer) -> u16 {
        let mut bank = SampleBank::new();
        let slot = bank.push(Arc::new(sample));
        handle
            .send(Command::SwapSampleBank(Arc::new(bank)))
            .unwrap();
        // One block applies the command and installs the empty-voice bank.
        let mut out = vec![0.0; 2 * 128];
        engine.process(&mut out, 128);
        slot
    }

    fn configure_pad(
        engine: &mut Engine,
        handle: &mut EngineHandle,
        pad: PadId,
        slot: u16,
        playback: Playback,
        choke: u8,
    ) {
        handle.send(Command::SetPadSample { pad, slot }).unwrap();
        handle
            .send(Command::SetPadPlayback { pad, playback })
            .unwrap();
        handle
            .send(Command::SetPadChokeGroup { pad, group: choke })
            .unwrap();
        let mut out = vec![0.0; 2 * 64];
        engine.process(&mut out, 64);
    }

    fn render(engine: &mut Engine, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; frames * 2];
        engine.process(&mut out, frames);
        out
    }

    #[test]
    fn sixty_four_voices_play_simultaneously() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 0.05));
        for pad in all_pads() {
            configure_pad(&mut engine, &mut handle, pad, slot, Playback::default(), 0);
        }
        // Four hits on each of 16 pads = 64 voices.
        for _ in 0..4 {
            for pad in all_pads() {
                handle
                    .note_on(pad, velocity(127), TriggerSource::Keyboard)
                    .unwrap();
            }
            let _ = render(&mut engine, 64);
        }
        assert_eq!(engine.active_voice_count(), 64);
        let out = render(&mut engine, 256);
        let peak = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak > 0.01, "64 voices produced no measurable signal");
    }

    #[test]
    fn oldest_voice_is_stolen_deterministically() {
        let (mut engine, mut handle) = engine(16);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 0.02));
        for pad in all_pads() {
            configure_pad(&mut engine, &mut handle, pad, slot, Playback::default(), 0);
        }
        for pad in all_pads() {
            handle
                .note_on(pad, velocity(127), TriggerSource::Keyboard)
                .unwrap();
        }
        let _ = render(&mut engine, 64);
        assert_eq!(engine.active_voice_count(), 16);
        let first = engine.voice_snapshot(0).unwrap();
        assert_eq!(first.pad, pad(0));

        // The 17th hit must target the oldest voice (index 0) and restart it
        // with the new pad after the steal fade.
        handle
            .note_on(pad(3), velocity(127), TriggerSource::Keyboard)
            .unwrap();
        let _ = render(&mut engine, 64);
        assert_eq!(engine.voice_snapshot(0).unwrap().pad, pad(0));
        assert!(engine.voice_snapshot(0).unwrap().active);
        let _ = render(&mut engine, 256);
        let snapshot = engine.voice_snapshot(0).unwrap();
        assert_eq!(snapshot.pad, pad(3), "oldest voice must be reused");
        assert_eq!(engine.active_voice_count(), 16);
    }

    #[test]
    fn trigger_is_sample_accurate_within_the_block() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 0.5));
        configure_pad(
            &mut engine,
            &mut handle,
            pad(0),
            slot,
            Playback::default(),
            0,
        );
        let start = handle.timestamp_now();
        let offset_frames = 100u64;
        let timestamp_us = (start + offset_frames) * 1_000_000 / u64::from(RATE);
        let expected_offset =
            (timestamp_us * u64::from(RATE) / 1_000_000).saturating_sub(start) as usize;
        handle
            .send_input(InputEvent::Trigger(
                drumpad_input::trigger::TriggerEvent::new(
                    PadId::FIRST,
                    Velocity::new(127).unwrap(),
                    TriggerSource::Keyboard,
                    timestamp_us,
                ),
            ))
            .unwrap();
        let out = render(&mut engine, 256);
        // Everything before the timestamped offset must be silent.
        for frame in 0..expected_offset {
            assert_eq!(out[frame * 2], 0.0, "silence before the trigger at {frame}");
        }
        assert!(
            out[expected_offset * 2 + 1].abs() < 1e-6,
            "first sample of the fade-in"
        );
        assert!(
            out[(expected_offset + 30) * 2].abs() > 0.01,
            "voice did not start at the timestamped offset: {}",
            out[(expected_offset + 30) * 2]
        );
    }

    #[test]
    fn gain_changes_are_smoothed_without_zipper_noise() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 1.0));
        configure_pad(
            &mut engine,
            &mut handle,
            pad(0),
            slot,
            Playback::default(),
            0,
        );
        handle
            .note_on(pad(0), velocity(127), TriggerSource::Keyboard)
            .unwrap();
        let _ = render(&mut engine, 480); // let the start fade finish
        handle
            .send(Command::SetPadGain {
                pad: pad(0),
                gain: 0.0,
            })
            .unwrap();
        let out = render(&mut engine, 2048);
        let mut max_step = 0.0f32;
        for frame in 1..1024 {
            let prev = out[(frame - 1) * 2];
            let now = out[frame * 2];
            max_step = max_step.max((now - prev).abs());
        }
        assert!(max_step < 0.01, "gain step was not smoothed: {max_step}");
    }

    #[test]
    fn equal_power_pan_law_for_mono_sources() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 0.5));
        configure_pad(
            &mut engine,
            &mut handle,
            pad(0),
            slot,
            Playback::default(),
            0,
        );
        handle
            .note_on(pad(0), velocity(127), TriggerSource::Keyboard)
            .unwrap();
        let _ = render(&mut engine, 480);
        let center = render(&mut engine, 16);
        let left = center[center.len() - 8];
        let right = center[center.len() - 7];
        assert!((left - 0.5 * FRAC_1_SQRT_2).abs() < 0.01, "left {left}");
        assert!((right - 0.5 * FRAC_1_SQRT_2).abs() < 0.01, "right {right}");

        handle
            .send(Command::SetPadPan {
                pad: pad(0),
                pan: -1.0,
            })
            .unwrap();
        let _ = render(&mut engine, 4800); // smooth to hard left
        let hard_left = render(&mut engine, 16);
        let l = hard_left[hard_left.len() - 8];
        let r = hard_left[hard_left.len() - 7];
        assert!(l > 0.45, "left {l}");
        assert!(r.abs() < 0.01, "right {r}");
    }

    #[test]
    fn choke_group_fades_the_competing_voice() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 0.5));
        configure_pad(
            &mut engine,
            &mut handle,
            pad(0),
            slot,
            Playback::default(),
            1,
        );
        configure_pad(
            &mut engine,
            &mut handle,
            pad(1),
            slot,
            Playback::default(),
            1,
        );

        handle
            .note_on(pad(0), velocity(127), TriggerSource::Keyboard)
            .unwrap();
        let _ = render(&mut engine, 2400);
        assert_eq!(engine.active_voice_count(), 1);
        handle
            .note_on(pad(1), velocity(127), TriggerSource::Keyboard)
            .unwrap();
        // Choke fade is 6 ms = 288 frames; one extra block is safe.
        let _ = render(&mut engine, 512);
        assert_eq!(engine.active_voice_count(), 1, "choked voice must stop");
        let snapshot = (0..engine.voices.len())
            .filter_map(|index| engine.voice_snapshot(index))
            .find(|voice| voice.active)
            .unwrap();
        assert_eq!(snapshot.pad, pad(1));
    }

    #[test]
    fn gate_release_stops_after_note_off() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 0.5));
        configure_pad(
            &mut engine,
            &mut handle,
            pad(0),
            slot,
            Playback {
                mode: PlayMode::Gate,
                loop_points: LoopPoints::default(),
            },
            0,
        );
        handle
            .note_on(pad(0), velocity(127), TriggerSource::Keyboard)
            .unwrap();
        let _ = render(&mut engine, 2400);
        assert_eq!(engine.active_voice_count(), 1);
        handle.note_off(pad(0), TriggerSource::Keyboard).unwrap();
        let _ = render(&mut engine, 256); // 5 ms release = 240 frames
        assert_eq!(engine.active_voice_count(), 0);
    }

    #[test]
    fn loop_mode_keeps_playing_across_blocks() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(1000, 0.25));
        configure_pad(
            &mut engine,
            &mut handle,
            pad(0),
            slot,
            Playback {
                mode: PlayMode::Loop,
                loop_points: LoopPoints {
                    start: 100,
                    end: 900,
                    crossfade: 32,
                },
            },
            0,
        );
        handle
            .note_on(pad(0), velocity(127), TriggerSource::Keyboard)
            .unwrap();
        for _ in 0..40 {
            let _ = render(&mut engine, 256);
        }
        assert_eq!(engine.active_voice_count(), 1);
    }

    #[test]
    fn panic_stops_all_voices() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 0.5));
        for pad in all_pads() {
            configure_pad(&mut engine, &mut handle, pad, slot, Playback::default(), 0);
            handle
                .note_on(pad, velocity(127), TriggerSource::Keyboard)
                .unwrap();
        }
        let _ = render(&mut engine, 512);
        assert_eq!(engine.active_voice_count(), 16);
        handle.panic().unwrap();
        let _ = render(&mut engine, 512);
        assert_eq!(engine.active_voice_count(), 0);
    }

    #[test]
    fn output_never_exceeds_zero_dbfs_with_many_loud_voices() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 1.0));
        for pad in all_pads() {
            configure_pad(&mut engine, &mut handle, pad, slot, Playback::default(), 0);
        }
        for _ in 0..4 {
            for pad in all_pads() {
                handle
                    .note_on(pad, velocity(127), TriggerSource::Keyboard)
                    .unwrap();
            }
            let _ = render(&mut engine, 64);
        }
        let out = render(&mut engine, 1024);
        let peak = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak <= 1.0, "output peaked at {peak}");
    }

    #[test]
    fn meters_follow_the_output() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 0.25));
        configure_pad(
            &mut engine,
            &mut handle,
            pad(0),
            slot,
            Playback::default(),
            0,
        );
        handle
            .note_on(pad(0), velocity(127), TriggerSource::Keyboard)
            .unwrap();
        let _ = render(&mut engine, 512);
        let values = handle.meter().values();
        assert!(values.peak_left > 0.1, "peak {}", values.peak_left);
        assert!(handle.meter().active_voices() >= 1);
    }

    #[test]
    fn voice_limit_change_does_not_require_reallocation() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        handle.send(Command::SetVoiceLimit { voices: 32 }).unwrap();
        let mut out = vec![0.0; 2 * 64];
        engine.process(&mut out, 64);
        assert_eq!(engine.voice_limit(), 32);
        handle
            .send(Command::SetVoiceLimit { voices: 1000 })
            .unwrap();
        engine.process(&mut out, 64);
        assert_eq!(engine.voice_limit(), MAX_VOICES);
        handle.send(Command::SetVoiceLimit { voices: 1 }).unwrap();
        engine.process(&mut out, 64);
        assert_eq!(engine.voice_limit(), MIN_VOICES);
    }

    #[test]
    fn retired_banks_are_returned_to_the_ui_thread() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(256, 0.25));
        assert_eq!(slot, 0);
        let mut bank = SampleBank::new();
        bank.push(Arc::new(constant_sample(512, 0.1)));
        handle
            .send(Command::SwapSampleBank(Arc::new(bank)))
            .unwrap();
        let _ = render(&mut engine, 128);
        assert_eq!(handle.collect_retired_banks(), 1);
    }

    #[test]
    fn pad_settings_round_trip_through_commands() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        handle
            .send(Command::SetPadGain {
                pad: pad(3),
                gain: 0.5,
            })
            .unwrap();
        handle
            .send(Command::SetPadPan {
                pad: pad(3),
                pan: 0.25,
            })
            .unwrap();
        handle
            .send(Command::SetPadTune {
                pad: pad(3),
                cents: -100,
            })
            .unwrap();
        handle
            .send(Command::SetPadChokeGroup {
                pad: pad(3),
                group: 2,
            })
            .unwrap();
        let mut out = vec![0.0; 64];
        engine.process(&mut out, 32);
        let settings = engine.pad_settings(pad(3)).unwrap();
        assert_eq!(settings.gain, 0.5);
        assert_eq!(settings.pan, 0.25);
        assert_eq!(settings.tune_cents, -100);
        assert_eq!(settings.choke_group, 2);
    }

    #[test]
    fn golden_one_shot_render_matches_expected_samples() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let source: Vec<f32> = (1..=64).map(|frame| frame as f32 * 0.01).collect();
        let slot = install(
            &mut engine,
            &mut handle,
            SampleBuffer::from_mono(source.clone(), RATE).unwrap(),
        );
        configure_pad(
            &mut engine,
            &mut handle,
            pad(0),
            slot,
            Playback::default(),
            0,
        );
        handle
            .send(Command::SetInterpolation(Interpolation::Linear))
            .unwrap();
        let _ = render(&mut engine, 32);

        handle
            .note_on(pad(0), velocity(127), TriggerSource::Keyboard)
            .unwrap();
        let out = render(&mut engine, 64);
        // Start fade is 0.5 ms = 24 frames at 48 kHz; equal-power centre is
        // 1/sqrt(2) per channel; the limiter is linear below -1 dBFS.
        let fade_frames = 24.0f32;
        for frame in 0..16 {
            let env = (frame as f32 / fade_frames).min(1.0);
            let expected = source[frame] * env * FRAC_1_SQRT_2;
            let actual = out[frame * 2];
            assert!(
                (actual - expected).abs() < 1e-4,
                "frame {frame}: expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn mode_change_applies_only_to_the_next_trigger() {
        let (mut engine, mut handle) = engine(DEFAULT_VOICES);
        let slot = install(&mut engine, &mut handle, constant_sample(4800, 0.2));
        configure_pad(
            &mut engine,
            &mut handle,
            pad(0),
            slot,
            Playback::default(),
            0,
        );
        handle
            .note_on(pad(0), velocity(127), TriggerSource::Keyboard)
            .unwrap();
        let _ = render(&mut engine, 64);
        assert_eq!(
            engine.voice_snapshot(0).unwrap().mode,
            PlayMode::OneShot,
            "first voice must keep the one-shot mode it started with"
        );

        handle
            .send(Command::SetPadPlayback {
                pad: pad(0),
                playback: Playback {
                    mode: PlayMode::Loop,
                    loop_points: LoopPoints {
                        start: 100,
                        end: 4000,
                        crossfade: 32,
                    },
                },
            })
            .unwrap();
        let _ = render(&mut engine, 64);
        assert_eq!(
            engine.voice_snapshot(0).unwrap().mode,
            PlayMode::OneShot,
            "mode changes must not retune a running voice"
        );

        // Let the first one-shot finish, then trigger again: now it loops.
        let _ = render(&mut engine, 4800);
        assert_eq!(engine.active_voice_count(), 0);
        handle
            .note_on(pad(0), velocity(127), TriggerSource::Keyboard)
            .unwrap();
        let _ = render(&mut engine, 64);
        assert_eq!(engine.voice_snapshot(0).unwrap().mode, PlayMode::Loop);
    }

    #[test]
    fn equal_timestamp_triggers_keep_fifo_order() {
        let (mut engine, mut handle) = engine(16);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 0.02));
        for pad in all_pads() {
            configure_pad(&mut engine, &mut handle, pad, slot, Playback::default(), 0);
        }
        // All 16 hits are stamped with the same playhead before the block.
        for pad in all_pads() {
            handle
                .note_on(pad, velocity(127), TriggerSource::Keyboard)
                .unwrap();
        }
        let _ = render(&mut engine, 64);
        let order: Vec<PadId> = (0..16)
            .map(|index| engine.voice_snapshot(index).unwrap().pad)
            .collect();
        let expected: Vec<PadId> = all_pads().collect();
        assert_eq!(order, expected, "same-timestamp triggers must stay FIFO");
    }

    #[test]
    fn oldest_stealing_advances_through_the_pool() {
        let (mut engine, mut handle) = engine(16);
        let slot = install(&mut engine, &mut handle, constant_sample(48_000, 0.02));
        for pad in all_pads() {
            configure_pad(&mut engine, &mut handle, pad, slot, Playback::default(), 0);
        }
        for pad in all_pads() {
            handle
                .note_on(pad, velocity(127), TriggerSource::Keyboard)
                .unwrap();
        }
        let _ = render(&mut engine, 64);

        // Two full batches of hits without a chance to finish the steal fades.
        // Each new hit must retire the next-oldest slot, not latch onto slot 0.
        for _ in 0..2 {
            for pad in all_pads() {
                handle
                    .note_on(pad, velocity(127), TriggerSource::Keyboard)
                    .unwrap();
            }
            let _ = render(&mut engine, 64);
        }
        let _ = render(&mut engine, 256); // let the last fades restart
        let orders: Vec<u64> = (0..16)
            .map(|index| engine.voice_snapshot(index).unwrap().started_at)
            .collect();
        let mut sorted = orders.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 16, "each voice must have a distinct order");
        assert!(
            orders.iter().all(|order| *order >= 32),
            "all slots must have advanced past the first two batches: {orders:?}"
        );
    }
}
