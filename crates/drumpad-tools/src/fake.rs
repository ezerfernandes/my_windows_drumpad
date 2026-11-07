//! Fake MIDI/audio integration harness (E11-T04).
//!
//! The harness drives the **real** engine, the **real** MIDI parser and
//! router, and the **real** keyboard map with scripted input. Only the audio
//! device is replaced by a caller-driven [`FakeAudioBackend`]: no thread, no
//! WASAPI handle, no wall clock. Every block is rendered on demand and
//! recorded, so a failure can print the script, the trigger log, and the voice
//! table for the offending block.
//!
//! Scripts are plain text with logical frame timestamps:
//!
//! ```text
//! # MIDI: frame  hex bytes
//! 0            90 24 64      # note-on, kick, velocity 100
//! 480          80 24 40      # note-off
//!
//! # Keyboard: frame  down|up  pad
//! 0    down 0
//! 480  up   0
//! ```
//!
//! Running status, note-on velocity zero, malformed data bytes, and system
//! realtime bytes are exercised through the real parser, so the script can
//! contain raw byte streams.

use std::error::Error;
use std::fmt;
use std::path::Path;

use drumpad_engine::mixer::EngineHandle;
use drumpad_input::keyboard::input::KeyboardAction;
use drumpad_input::keyboard::input::KeyboardInput;
use drumpad_input::keyboard::keys::PhysicalKey;
use drumpad_input::keyboard::profile::KeyboardProfile;
use drumpad_input::midi::router::{MidiIgnoreReason, MidiOutput, MidiRouter};
use drumpad_input::trigger::{InputEvent, TriggerSource};

use crate::fixture::FixtureBank;
use crate::offline::{OfflineSession, RenderConfig, RenderError, VoiceTraceEntry};
use crate::sequence::PAD_COUNT;

/// One raw MIDI byte chunk scheduled at a logical frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiScriptEvent {
    /// Logical frame at which the bytes are delivered.
    pub at_frame: u64,
    /// Raw MIDI bytes.
    pub bytes: Vec<u8>,
}

/// A scripted MIDI byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiScript {
    events: Vec<MidiScriptEvent>,
    text: String,
}

impl MidiScript {
    /// Parse `<frame> <hex byte>...` lines. Blank lines and `#` comments are
    /// ignored. `0x` prefixes are accepted.
    pub fn parse(text: &str) -> Result<Self, FakeScriptError> {
        let mut events = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line_number = index + 1;
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let frame_text = parts
                .next()
                .ok_or_else(|| FakeScriptError::new(line_number, "missing frame"))?;
            let at_frame = frame_text.parse::<u64>().map_err(|_| {
                FakeScriptError::new(
                    line_number,
                    format!("frame {frame_text:?} is not an integer"),
                )
            })?;
            let mut bytes = Vec::new();
            for token in parts {
                let token = token.trim_start_matches("0x").trim_start_matches("0X");
                if token.is_empty() || token.len() > 2 {
                    return Err(FakeScriptError::new(
                        line_number,
                        format!("{token:?} is not one byte of hex"),
                    ));
                }
                let byte = u8::from_str_radix(token, 16).map_err(|_| {
                    FakeScriptError::new(line_number, format!("{token:?} is not one byte of hex"))
                })?;
                bytes.push(byte);
            }
            if bytes.is_empty() {
                return Err(FakeScriptError::new(
                    line_number,
                    "a MIDI line needs at least one byte",
                ));
            }
            events.push(MidiScriptEvent { at_frame, bytes });
        }
        events.sort_by_key(|event| event.at_frame);
        Ok(Self {
            events,
            text: text.to_owned(),
        })
    }

    /// Load from a file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, FakeScriptError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|error| FakeScriptError::io(path.display().to_string(), error.to_string()))?;
        Self::parse(&text)
    }

    /// Scheduled events.
    pub fn events(&self) -> &[MidiScriptEvent] {
        &self.events
    }

    /// Original text, for failure output.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Last scheduled frame.
    pub fn last_frame(&self) -> u64 {
        self.events.last().map(|event| event.at_frame).unwrap_or(0)
    }
}

/// Direction of a scripted key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyDirection {
    /// Key pressed.
    Down,
    /// Key released.
    Up,
}

/// One scripted key action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardScriptEvent {
    /// Logical frame at which the key changes.
    pub at_frame: u64,
    /// Press or release.
    pub direction: KeyDirection,
    /// Pad the key maps to through the Finding 35-key profile.
    pub pad: u8,
}

/// A scripted keyboard stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardScript {
    events: Vec<KeyboardScriptEvent>,
    text: String,
}

impl KeyboardScript {
    /// Parse `<frame> down|up <pad>` lines.
    pub fn parse(text: &str) -> Result<Self, FakeScriptError> {
        let mut events = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line_number = index + 1;
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let frame_text = parts
                .next()
                .ok_or_else(|| FakeScriptError::new(line_number, "missing frame"))?;
            let at_frame = frame_text.parse::<u64>().map_err(|_| {
                FakeScriptError::new(
                    line_number,
                    format!("frame {frame_text:?} is not an integer"),
                )
            })?;
            let direction = match parts.next() {
                Some("down") => KeyDirection::Down,
                Some("up") => KeyDirection::Up,
                Some(other) => {
                    return Err(FakeScriptError::new(
                        line_number,
                        format!("direction {other:?} must be down or up"),
                    ));
                }
                None => return Err(FakeScriptError::new(line_number, "missing direction")),
            };
            let pad_text = parts
                .next()
                .ok_or_else(|| FakeScriptError::new(line_number, "missing pad"))?;
            let pad = pad_text.parse::<u8>().map_err(|_| {
                FakeScriptError::new(line_number, format!("pad {pad_text:?} is not an integer"))
            })?;
            if pad >= PAD_COUNT {
                return Err(FakeScriptError::new(
                    line_number,
                    format!("pad {pad} is outside 0..{PAD_COUNT}"),
                ));
            }
            if let Some(extra) = parts.next() {
                return Err(FakeScriptError::new(
                    line_number,
                    format!("unexpected trailing token {extra:?}"),
                ));
            }
            events.push(KeyboardScriptEvent {
                at_frame,
                direction,
                pad,
            });
        }
        events.sort_by_key(|event| event.at_frame);
        Ok(Self {
            events,
            text: text.to_owned(),
        })
    }

    /// Load from a file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, FakeScriptError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|error| FakeScriptError::io(path.display().to_string(), error.to_string()))?;
        Self::parse(&text)
    }

    /// Scheduled events.
    pub fn events(&self) -> &[KeyboardScriptEvent] {
        &self.events
    }

    /// Original text, for failure output.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Last scheduled frame.
    pub fn last_frame(&self) -> u64 {
        self.events.last().map(|event| event.at_frame).unwrap_or(0)
    }
}

/// Errors from fake scripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeScriptError {
    /// 1-based line number, or 0 for an IO error.
    pub line: usize,
    /// Human-readable reason.
    pub message: String,
}

impl FakeScriptError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }

    fn io(path: String, message: String) -> Self {
        Self {
            line: 0,
            message: format!("cannot read {path}: {message}"),
        }
    }
}

impl fmt::Display for FakeScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(formatter, "{}", self.message)
        } else {
            write!(formatter, "line {}: {}", self.line, self.message)
        }
    }
}

impl Error for FakeScriptError {}

/// A caller-driven audio backend for tests.
///
/// It owns an [`OfflineSession`] and renders exactly one block per
/// [`FakeAudioBackend::process_block`] call. Every block is recorded so failure
/// output can quote the signal. There is no device, thread, or sleep.
pub struct FakeAudioBackend {
    session: OfflineSession,
    blocks: Vec<Vec<f32>>,
    voice_trace: Vec<VoiceTraceEntry>,
}

impl FakeAudioBackend {
    /// Create a backend around the standard fixture bank.
    pub fn new(config: RenderConfig) -> Result<Self, RenderError> {
        Self::with_bank(config, FixtureBank::standard())
    }

    /// Create a backend around a caller-provided bank.
    pub fn with_bank(config: RenderConfig, bank: FixtureBank) -> Result<Self, RenderError> {
        let session = OfflineSession::with_bank(config, bank)?;
        Ok(Self {
            session,
            blocks: Vec::new(),
            voice_trace: Vec::new(),
        })
    }

    /// Queue an input event through the engine's canonical input path.
    pub fn send_input(&mut self, event: InputEvent) -> Result<(), RenderError> {
        self.handle_mut()
            .send_input(event)
            .map_err(|error| RenderError::Queue(format!("{error:?}")))
    }

    /// Mutable engine handle.
    pub fn handle_mut(&mut self) -> &mut EngineHandle {
        self.session.handle_mut()
    }

    /// Immutable engine handle.
    pub fn handle(&self) -> &EngineHandle {
        self.session.handle()
    }

    /// Triggers dropped because the input queue was full.
    pub fn dropped_triggers(&self) -> u64 {
        self.handle().dropped_triggers()
    }

    /// Commands dropped because the command queue was full.
    pub fn dropped_commands(&self) -> u64 {
        self.handle().dropped_commands()
    }

    /// Convert a logical frame to the engine input clock.
    pub fn frame_to_us(&self, frame: u64) -> u64 {
        self.session.frame_to_us(frame)
    }

    /// Block size in frames.
    pub fn block_frames(&self) -> usize {
        self.session.config().block_frames
    }

    /// Render one block and record it.
    pub fn process_block(&mut self) -> &[f32] {
        let block = self.session.process_block();
        self.blocks.push(block);
        let block_index = self.blocks.len() - 1;
        let counts = self.session.active_pad_counts();
        let active = counts.iter().copied().sum();
        self.voice_trace.push(VoiceTraceEntry {
            block: block_index,
            frame: (block_index as u64 + 1) * self.block_frames() as u64,
            active,
            per_pad: counts,
        });
        self.blocks.last().expect("just pushed")
    }

    /// Blocks rendered so far.
    pub fn blocks(&self) -> &[Vec<f32>] {
        &self.blocks
    }

    /// Voice trace per rendered block.
    pub fn voice_trace(&self) -> &[VoiceTraceEntry] {
        &self.voice_trace
    }

    /// Active voice counts per pad right now.
    pub fn active_pad_counts(&self) -> [u16; 16] {
        self.session.active_pad_counts()
    }
}

/// One trigger or release observed by the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerLogEntry {
    /// Logical frame at which the event was delivered.
    pub at_frame: u64,
    /// Pad targeted.
    pub pad: u8,
    /// Velocity for a trigger, `None` for a release.
    pub velocity: Option<u8>,
    /// Capture source.
    pub source: TriggerSource,
}

/// One rendered block, summarized.
#[derive(Debug, Clone, PartialEq)]
pub struct HarnessBlock {
    /// Block index.
    pub block: usize,
    /// First logical frame of the block.
    pub frame: u64,
    /// Peak absolute sample.
    pub peak: f32,
    /// Root mean square.
    pub rms: f32,
    /// Active voices after the block.
    pub active: u16,
    /// Active voices per pad after the block.
    pub per_pad: [u16; 16],
}

/// The full result of a fake-harness run.
#[derive(Debug, Clone)]
pub struct HarnessReport {
    /// Configuration used.
    pub config: RenderConfig,
    /// MIDI script text.
    pub midi_script: String,
    /// Keyboard script text.
    pub keyboard_script: String,
    /// Trigger/release log in delivery order.
    pub trigger_log: Vec<TriggerLogEntry>,
    /// MIDI messages the router deliberately ignored.
    pub midi_ignored: Vec<(u64, MidiIgnoreReason)>,
    /// Per-block signal and voice summary.
    pub blocks: Vec<HarnessBlock>,
    /// Render error, if the run stopped early.
    pub error: Option<String>,
}

impl HarnessReport {
    /// True when every block was finite and at least one voice played.
    pub fn is_sane(&self) -> bool {
        self.error.is_none()
            && self.blocks.iter().all(|block| block.peak.is_finite())
            && self.blocks.iter().any(|block| block.peak > 0.0)
    }

    /// Voices still active after the last block.
    pub fn final_active_voices(&self) -> u16 {
        self.blocks.last().map(|block| block.active).unwrap_or(0)
    }

    /// Per-pad active voices after the last block.
    pub fn final_per_pad(&self) -> [u16; 16] {
        self.blocks
            .last()
            .map(|block| block.per_pad)
            .unwrap_or([0; 16])
    }

    /// Number of triggers logged for `pad`.
    pub fn trigger_count(&self, pad: u8) -> usize {
        self.trigger_log
            .iter()
            .filter(|entry| entry.pad == pad && entry.velocity.is_some())
            .count()
    }

    /// Number of releases logged for `pad`.
    pub fn release_count(&self, pad: u8) -> usize {
        self.trigger_log
            .iter()
            .filter(|entry| entry.pad == pad && entry.velocity.is_none())
            .count()
    }

    /// Render a failure dump with the scripts, trigger log, and the last five
    /// blocks of the voice table.
    pub fn failure_dump(&self) -> String {
        let mut out = String::new();
        out.push_str("=== fake MIDI/keyboard harness failure ===\n");
        if let Some(error) = &self.error {
            out.push_str(&format!("error: {error}\n"));
        }
        out.push_str(&format!(
            "config: sample_rate={} block_frames={} voice_limit={} limiter={}\n",
            self.config.sample_rate,
            self.config.block_frames,
            self.config.voice_limit,
            self.config.limiter
        ));
        out.push_str("\n--- MIDI script ---\n");
        out.push_str(&self.midi_script);
        out.push_str("\n--- keyboard script ---\n");
        out.push_str(&self.keyboard_script);
        out.push_str("\n--- trigger log ---\n");
        for entry in &self.trigger_log {
            match entry.velocity {
                Some(velocity) => out.push_str(&format!(
                    "{} trigger pad {} velocity {} ({})\n",
                    entry.at_frame,
                    entry.pad,
                    velocity,
                    entry.source.as_str()
                )),
                None => out.push_str(&format!(
                    "{} release pad {} ({})\n",
                    entry.at_frame,
                    entry.pad,
                    entry.source.as_str()
                )),
            }
        }
        if !self.midi_ignored.is_empty() {
            out.push_str("\n--- ignored MIDI ---\n");
            for (frame, reason) in &self.midi_ignored {
                out.push_str(&format!("{frame} {reason:?}\n"));
            }
        }
        out.push_str("\n--- voice table (last blocks) ---\n");
        out.push_str("block frame active per-pad [0..15]\n");
        for block in self.blocks.iter().rev().take(5).rev() {
            out.push_str(&format!(
                "{} {} {} {:?}\n",
                block.block, block.frame, block.active, block.per_pad
            ));
        }
        out
    }
}

/// The fake MIDI/keyboard harness.
pub struct FakeHarness {
    backend: FakeAudioBackend,
    midi: MidiRouter,
    keyboard: KeyboardInput,
    midi_script: MidiScript,
    keyboard_script: KeyboardScript,
    trigger_log: Vec<TriggerLogEntry>,
    midi_ignored: Vec<(u64, MidiIgnoreReason)>,
    blocks: Vec<HarnessBlock>,
    midi_cursor: usize,
    keyboard_cursor: usize,
}

impl FakeHarness {
    /// Build a harness around the standard fixture bank.
    pub fn new(
        config: RenderConfig,
        midi_script: MidiScript,
        keyboard_script: KeyboardScript,
    ) -> Result<Self, RenderError> {
        Self::with_bank(
            config,
            FixtureBank::standard(),
            midi_script,
            keyboard_script,
        )
    }

    /// Build a harness around a caller-provided bank.
    pub fn with_bank(
        config: RenderConfig,
        bank: FixtureBank,
        midi_script: MidiScript,
        keyboard_script: KeyboardScript,
    ) -> Result<Self, RenderError> {
        let backend = FakeAudioBackend::with_bank(config, bank)?;
        let keyboard = KeyboardInput::default_with_profile(&KeyboardProfile::finding_35_key());
        Ok(Self {
            backend,
            midi: MidiRouter::nitro_max(),
            keyboard,
            midi_script,
            keyboard_script,
            trigger_log: Vec::new(),
            midi_ignored: Vec::new(),
            blocks: Vec::new(),
            midi_cursor: 0,
            keyboard_cursor: 0,
        })
    }

    /// Total logical frames this harness will render.
    pub fn total_frames(&self) -> u64 {
        let last = self
            .midi_script
            .last_frame()
            .max(self.keyboard_script.last_frame());
        let block_frames = self.backend.block_frames() as u64;
        last.div_ceil(block_frames)
            .saturating_mul(block_frames)
            .saturating_add(self.backend.session().config().tail_frames)
    }

    /// Run to completion and return the report.
    pub fn run(mut self) -> HarnessReport {
        let block_frames = self.backend.block_frames() as u64;
        let total = self.total_frames();
        let blocks = total.div_ceil(block_frames).max(1) as usize;
        let mut error = None;
        for block in 0..blocks {
            let block_start = block as u64 * block_frames;
            let block_end = block_start + block_frames;
            if let Err(render_error) = self.deliver_input(block_start, block_end) {
                error = Some(render_error.to_string());
                break;
            }
            let output = self.backend.process_block();
            let peak = output
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            let rms = (output
                .iter()
                .map(|sample| f64::from(*sample) * f64::from(*sample))
                .sum::<f64>()
                / output.len().max(1) as f64)
                .sqrt() as f32;
            let counts = self.backend.active_pad_counts();
            let active = counts.iter().copied().sum();
            self.blocks.push(HarnessBlock {
                block,
                frame: block_start,
                peak,
                rms,
                active,
                per_pad: counts,
            });
        }
        HarnessReport {
            config: *self.backend.session().config(),
            midi_script: self.midi_script.text().to_owned(),
            keyboard_script: self.keyboard_script.text().to_owned(),
            trigger_log: self.trigger_log,
            midi_ignored: self.midi_ignored,
            blocks: self.blocks,
            error,
        }
    }

    fn deliver_input(&mut self, block_start: u64, block_end: u64) -> Result<(), RenderError> {
        // MIDI bytes that fall in this block, delivered at their exact frame.
        let mut midi_work: Vec<(u64, Vec<u8>)> = Vec::new();
        while self.midi_cursor < self.midi_script.events.len() {
            let event = &self.midi_script.events[self.midi_cursor];
            if event.at_frame >= block_end {
                break;
            }
            if event.at_frame >= block_start {
                midi_work.push((event.at_frame, event.bytes.clone()));
            }
            self.midi_cursor += 1;
        }
        for (frame, bytes) in midi_work {
            let timestamp_us = self.backend.frame_to_us(frame);
            let mut outputs = Vec::new();
            self.midi.process_bytes(&bytes, timestamp_us, &mut outputs);
            for output in outputs {
                self.forward_midi(output, frame)?;
            }
        }

        // Keyboard events in the same block.
        let mut keyboard_work = Vec::new();
        while self.keyboard_cursor < self.keyboard_script.events.len() {
            let event = self.keyboard_script.events[self.keyboard_cursor];
            if event.at_frame >= block_end {
                break;
            }
            if event.at_frame >= block_start {
                keyboard_work.push(event);
            }
            self.keyboard_cursor += 1;
        }
        for event in keyboard_work {
            let timestamp_us = self.backend.frame_to_us(event.at_frame);
            let key = PhysicalKey::PAD_BLOCK
                .get(event.pad as usize)
                .copied()
                .ok_or_else(|| {
                    RenderError::Config(format!("pad {} has no default key", event.pad))
                })?;
            let action = match event.direction {
                KeyDirection::Down => self.keyboard.key_down(key, timestamp_us),
                KeyDirection::Up => self.keyboard.key_up(key, timestamp_us),
            };
            match action {
                KeyboardAction::Trigger(trigger) => {
                    self.log_trigger(
                        event.at_frame,
                        trigger.pad.get(),
                        Some(trigger.velocity.get()),
                        trigger.source,
                    );
                    self.backend.send_input(InputEvent::Trigger(trigger))?;
                }
                KeyboardAction::Release(release) => {
                    self.log_trigger(event.at_frame, release.pad.get(), None, release.source);
                    self.backend.send_input(InputEvent::Release(release))?;
                }
                KeyboardAction::Transport(_) | KeyboardAction::Ignored(_) => {}
            }
        }
        Ok(())
    }

    fn forward_midi(&mut self, output: MidiOutput, frame: u64) -> Result<(), RenderError> {
        match output {
            MidiOutput::Trigger(trigger) => {
                self.log_trigger(
                    frame,
                    trigger.pad.get(),
                    Some(trigger.velocity.get()),
                    trigger.source,
                );
                self.backend.send_input(InputEvent::Trigger(trigger))?;
            }
            MidiOutput::Release(release) => {
                self.log_trigger(frame, release.pad.get(), None, release.source);
                self.backend.send_input(InputEvent::Release(release))?;
            }
            MidiOutput::HiHat { event, .. } => {
                self.log_trigger(
                    frame,
                    event.pad.get(),
                    Some(event.velocity.get()),
                    event.source,
                );
                self.backend.send_input(InputEvent::Trigger(event))?;
            }
            MidiOutput::Learned { .. } => {}
            MidiOutput::Ignored(reason) => self.midi_ignored.push((frame, reason)),
        }
        Ok(())
    }

    fn log_trigger(&mut self, at_frame: u64, pad: u8, velocity: Option<u8>, source: TriggerSource) {
        self.trigger_log.push(TriggerLogEntry {
            at_frame,
            pad,
            velocity,
            source,
        });
    }
}

impl FakeAudioBackend {
    /// Session access for tests that need the fixture bank or config.
    pub fn session(&self) -> &OfflineSession {
        &self.session
    }
}

/// Convenience: parse both scripts and run the harness in one call.
pub fn run_scripts(
    config: RenderConfig,
    midi_text: &str,
    keyboard_text: &str,
) -> Result<HarnessReport, Box<dyn Error>> {
    let midi_script = MidiScript::parse(midi_text)?;
    let keyboard_script = KeyboardScript::parse(keyboard_text)?;
    let harness = FakeHarness::new(config, midi_script, keyboard_script)
        .map_err(|error| -> Box<dyn Error> { Box::new(error) })?;
    Ok(harness.run())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIDI: &str = "\
# note-on kick, then velocity-zero note-off through running status
0   90 24 64
480 24 00
960 90 26 50
1200 80 26 40
";

    const KEYS: &str = "\
0   down 0
240 up   0
480 down 1
960 up   1
";

    #[test]
    fn midi_script_parses_hex_with_comments() {
        let script = MidiScript::parse(MIDI).expect("parse");
        assert_eq!(script.events().len(), 4);
        assert_eq!(script.events()[0].bytes, vec![0x90, 0x24, 0x64]);
        assert_eq!(script.events()[1].bytes, vec![0x24, 0x00]);
    }

    #[test]
    fn keyboard_script_parses_directions_and_rejects_bad_pads() {
        let script = KeyboardScript::parse(KEYS).expect("parse");
        assert_eq!(script.events().len(), 4);
        let error = KeyboardScript::parse("0 down 16\n").expect_err("bad pad");
        assert!(error.message.contains("outside"), "{error}");
    }

    #[test]
    fn harness_runs_midi_and_keyboard_without_hardware() {
        let report = run_scripts(RenderConfig::default(), MIDI, KEYS).expect("run");
        assert!(report.is_sane(), "{}", report.failure_dump());
        assert_eq!(report.trigger_count(0), 2, "{}", report.failure_dump());
        assert_eq!(report.trigger_count(1), 2, "{}", report.failure_dump());
        assert_eq!(report.release_count(0), 2);
        assert_eq!(report.final_active_voices(), 0, "stuck voices");
    }

    #[test]
    fn velocity_zero_becomes_a_release() {
        let report = run_scripts(RenderConfig::default(), MIDI, "").expect("run");
        assert_eq!(report.trigger_count(0), 1);
        assert_eq!(report.release_count(0), 1);
        assert_eq!(report.final_active_voices(), 0);
    }

    #[test]
    fn malformed_bytes_are_reported_not_panicked() {
        let report =
            run_scripts(RenderConfig::default(), "0 24 64\n0 90 24 64\n", "").expect("run");
        assert!(report.is_sane(), "{}", report.failure_dump());
        assert!(
            !report.midi_ignored.is_empty(),
            "malformed byte was not reported"
        );
    }

    #[test]
    fn failure_dump_contains_script_log_and_voice_table() {
        let report = run_scripts(RenderConfig::default(), MIDI, KEYS).expect("run");
        let dump = report.failure_dump();
        assert!(dump.contains("MIDI script"));
        assert!(dump.contains("trigger log"));
        assert!(dump.contains("voice table"));
    }

    #[test]
    fn no_stuck_voice_after_gate_release() {
        let report =
            run_scripts(RenderConfig::default(), "0 90 24 80\n480 80 24 40\n", "").expect("run");
        assert_eq!(report.final_active_voices(), 0, "{}", report.failure_dump());
    }
}
