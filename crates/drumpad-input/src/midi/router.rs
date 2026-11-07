//! MIDI routing: parser -> channel filter -> learn -> velocity -> note map and
//! hi-hat engine.
//!
//! One router owns all mutable MIDI state. The app feeds it bytes from the
//! driver callback and drains [`MidiOutput`] values into the shared queue (or
//! into the UI during learn). The steady-state path does not allocate after
//! construction: the parser scratch buffer and the held-note list are
//! preallocated. A burst larger than the scratch buffer can still grow it, and
//! the caller owns the output vector.

use crate::midi::hihat::{HiHatArticulation, HiHatConfig, HiHatEngine, HiHatSound};
use crate::midi::learn::{CapturedControl, LearnOutcome, MidiLearn};
use crate::midi::note_map::MidiNoteMap;
use crate::midi::parse::{ChannelFilter, IgnoreKind, MidiEvent, MidiParser};
use crate::midi::velocity::{CurveError, MidiVelocityConfig, VelocityCurveTable};
use crate::trigger::{PadId, ReleaseEvent, TriggerEvent, TriggerSource, Velocity};

/// High-level MIDI configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiRouterConfig {
    /// Channel filter applied to every message.
    pub channel_filter: ChannelFilter,
    /// Velocity curve and per-pad sensitivity.
    pub velocity: MidiVelocityConfig,
    /// Hi-hat note/CC configuration.
    pub hihat: HiHatConfig,
    /// Route hi-hat notes through the articulation engine.
    pub hihat_enabled: bool,
}

impl Default for MidiRouterConfig {
    fn default() -> Self {
        Self {
            channel_filter: ChannelFilter::All,
            velocity: MidiVelocityConfig::default(),
            hihat: HiHatConfig::default(),
            hihat_enabled: true,
        }
    }
}

/// Why a message produced no trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiIgnoreReason {
    /// The channel filter rejected the message.
    ChannelFiltered,
    /// No note map entry for the note.
    UnmappedNote,
    /// A controller the trigger path does not use.
    UnmappedControl,
    /// The parser classified the message as ignored.
    Parser(IgnoreKind),
}

/// One routed result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiOutput {
    /// An ordinary pad hit.
    Trigger(TriggerEvent),
    /// A gate-mode release.
    Release(ReleaseEvent),
    /// A hi-hat hit; the pad trigger is included for the queue, the
    /// articulation tells the engine which layer to choose.
    HiHat {
        /// Pad event to queue.
        event: TriggerEvent,
        /// Articulation selected by notes or CC.
        articulation: HiHatArticulation,
        /// Sound family (hat, pedal chick, splash).
        sound: HiHatSound,
    },
    /// A MIDI-learn capture; do not sound it.
    Learned {
        /// Pad being learned.
        pad: PadId,
        /// Captured control.
        control: CapturedControl,
    },
    /// Nothing fired, for the stated reason.
    Ignored(MidiIgnoreReason),
}

impl MidiOutput {
    /// Pad the output applies to, if any.
    #[must_use]
    pub fn pad(self) -> Option<PadId> {
        match self {
            Self::Trigger(event) => Some(event.pad),
            Self::Release(event) => Some(event.pad),
            Self::HiHat { event, .. } => Some(event.pad),
            Self::Learned { pad, .. } => Some(pad),
            Self::Ignored(_) => None,
        }
    }

    /// Shared queue event for normal triggers and hi-hat hits.
    #[must_use]
    pub fn event(self) -> Option<crate::trigger::InputEvent> {
        match self {
            Self::Trigger(event) => Some(event.into()),
            Self::Release(event) => Some(event.into()),
            Self::HiHat { event, .. } => Some(event.into()),
            Self::Learned { .. } | Self::Ignored(_) => None,
        }
    }
}

/// Stateful MIDI trigger router.
#[derive(Debug, Clone)]
pub struct MidiRouter {
    parser: MidiParser,
    scratch: Vec<MidiEvent>,
    note_map: MidiNoteMap,
    channel_filter: ChannelFilter,
    velocity_config: MidiVelocityConfig,
    velocity_table: VelocityCurveTable,
    hihat: HiHatEngine,
    hihat_enabled: bool,
    learn: MidiLearn,
    held: Vec<(u8, u8, PadId)>,
}

impl MidiRouter {
    /// Build a router, compiling the velocity curve once.
    pub fn new(note_map: MidiNoteMap, config: MidiRouterConfig) -> Result<Self, CurveError> {
        let velocity_table = config.velocity.compile()?;
        Ok(Self {
            parser: MidiParser::new(),
            scratch: Vec::with_capacity(32),
            note_map,
            channel_filter: config.channel_filter,
            velocity_config: config.velocity,
            velocity_table,
            hihat: HiHatEngine::new(config.hihat),
            hihat_enabled: config.hihat_enabled,
            learn: MidiLearn::new(),
            held: Vec::with_capacity(32),
        })
    }

    /// Router with the Nitro Max template and default configuration.
    pub fn nitro_max() -> Self {
        Self::new(
            MidiNoteMap::nitro_max_template(),
            MidiRouterConfig::default(),
        )
        .expect("default curve compiles")
    }

    /// Active note map.
    #[must_use]
    pub fn note_map(&self) -> &MidiNoteMap {
        &self.note_map
    }

    /// Replace the note map; held notes keep the pad they were pressed on so a
    /// map change cannot strand a gate.
    pub fn set_note_map(&mut self, note_map: MidiNoteMap) {
        self.note_map = note_map;
    }

    /// Active channel filter.
    #[must_use]
    pub fn channel_filter(&self) -> &ChannelFilter {
        &self.channel_filter
    }

    /// Replace the channel filter.
    pub fn set_channel_filter(&mut self, filter: ChannelFilter) {
        self.channel_filter = filter;
    }

    /// Active velocity configuration.
    #[must_use]
    pub fn velocity_config(&self) -> &MidiVelocityConfig {
        &self.velocity_config
    }

    /// Replace the velocity configuration and recompile its table.
    pub fn set_velocity_config(&mut self, config: MidiVelocityConfig) -> Result<(), CurveError> {
        self.velocity_table = config.compile()?;
        self.velocity_config = config;
        Ok(())
    }

    /// Mid-learn session.
    #[must_use]
    pub fn learn(&self) -> &MidiLearn {
        &self.learn
    }

    /// Mutable learn session for arming and disarming.
    pub fn learn_mut(&mut self) -> &mut MidiLearn {
        &mut self.learn
    }

    /// Hi-hat engine state.
    #[must_use]
    pub fn hihat(&self) -> &HiHatEngine {
        &self.hihat
    }

    /// Pad used for hi-hat articulations, if the map has one.
    #[must_use]
    pub fn hihat_pad(&self) -> Option<PadId> {
        let config = self.hihat.config();
        self.note_map
            .pad_for(config.open_note)
            .or_else(|| self.note_map.pad_for(config.closed_note))
    }

    /// Parse and route a chunk of bytes, appending results to `out`.
    pub fn process_bytes(&mut self, bytes: &[u8], timestamp_us: u64, out: &mut Vec<MidiOutput>) {
        let mut events = std::mem::take(&mut self.scratch);
        events.clear();
        self.parser.push_slice(bytes, &mut events);
        for &event in &events {
            self.route(event, timestamp_us, out);
        }
        events.clear();
        self.scratch = events;
    }

    /// Route bytes and record every parsed message in `monitor` first.
    pub fn process_bytes_monitored(
        &mut self,
        bytes: &[u8],
        timestamp_us: u64,
        monitor: &mut crate::midi::monitor::MidiMonitor,
        out: &mut Vec<MidiOutput>,
    ) {
        let mut events = std::mem::take(&mut self.scratch);
        events.clear();
        self.parser.push_slice(bytes, &mut events);
        for &event in &events {
            monitor.record(timestamp_us, event);
            self.route(event, timestamp_us, out);
        }
        events.clear();
        self.scratch = events;
    }

    /// Route one already-parsed message.
    pub fn route(&mut self, event: MidiEvent, timestamp_us: u64, out: &mut Vec<MidiOutput>) {
        match event {
            MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } => self.note_on(channel, note, velocity, timestamp_us, out),
            MidiEvent::NoteOff { channel, note } => self.note_off(channel, note, timestamp_us, out),
            MidiEvent::ControlChange {
                channel,
                controller,
                value,
            } => self.control_change(channel, controller, value, timestamp_us, out),
            MidiEvent::Ignored { kind, .. } => {
                out.push(MidiOutput::Ignored(MidiIgnoreReason::Parser(kind)))
            }
        }
    }

    fn note_on(
        &mut self,
        channel: u8,
        note: u8,
        velocity: Velocity,
        timestamp_us: u64,
        out: &mut Vec<MidiOutput>,
    ) {
        if !self.channel_filter.accepts(channel) {
            out.push(MidiOutput::Ignored(MidiIgnoreReason::ChannelFiltered));
            return;
        }
        if self.learn.armed().is_some() {
            let outcome = self.learn.observe(MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            });
            if let LearnOutcome::Captured { pad, control } = outcome {
                out.push(MidiOutput::Learned { pad, control });
                return;
            }
        }
        let Some(mapping) = self.note_map.mapping(note) else {
            out.push(MidiOutput::Ignored(MidiIgnoreReason::UnmappedNote));
            return;
        };
        let pad = mapping.pad;
        let shaped = self
            .velocity_config
            .map(&self.velocity_table, velocity, pad);

        if self.hihat_enabled {
            if let Some(trigger) = self.hihat.note(note, shaped) {
                self.remember_held(channel, note, pad);
                out.push(MidiOutput::HiHat {
                    event: TriggerEvent::new(
                        pad,
                        trigger.velocity,
                        TriggerSource::Midi,
                        timestamp_us,
                    ),
                    articulation: trigger.articulation,
                    sound: trigger.sound,
                });
                return;
            }
        }

        self.remember_held(channel, note, pad);
        out.push(MidiOutput::Trigger(TriggerEvent::new(
            pad,
            shaped,
            TriggerSource::Midi,
            timestamp_us,
        )));
    }

    fn note_off(&mut self, channel: u8, note: u8, timestamp_us: u64, out: &mut Vec<MidiOutput>) {
        if !self.channel_filter.accepts(channel) {
            out.push(MidiOutput::Ignored(MidiIgnoreReason::ChannelFiltered));
            return;
        }
        let Some(index) = self.held.iter().position(|(held_channel, held_note, _)| {
            *held_channel == channel && *held_note == note
        }) else {
            out.push(MidiOutput::Ignored(MidiIgnoreReason::UnmappedNote));
            return;
        };
        let (_, _, pad) = self.held.swap_remove(index);
        out.push(MidiOutput::Release(ReleaseEvent::new(
            pad,
            TriggerSource::Midi,
            timestamp_us,
        )));
    }

    fn control_change(
        &mut self,
        channel: u8,
        controller: u8,
        value: u8,
        timestamp_us: u64,
        out: &mut Vec<MidiOutput>,
    ) {
        if !self.channel_filter.accepts(channel) {
            out.push(MidiOutput::Ignored(MidiIgnoreReason::ChannelFiltered));
            return;
        }
        if self.learn.armed().is_some() {
            let outcome = self.learn.observe(MidiEvent::ControlChange {
                channel,
                controller,
                value,
            });
            if let LearnOutcome::Captured { pad, control } = outcome {
                out.push(MidiOutput::Learned { pad, control });
                return;
            }
        }
        if !self.hihat_enabled {
            out.push(MidiOutput::Ignored(MidiIgnoreReason::UnmappedControl));
            return;
        }
        let Some(trigger) = self.hihat.control_change(controller, value) else {
            out.push(MidiOutput::Ignored(MidiIgnoreReason::UnmappedControl));
            return;
        };
        let Some(pad) = self.hihat_pad() else {
            out.push(MidiOutput::Ignored(MidiIgnoreReason::UnmappedNote));
            return;
        };
        out.push(MidiOutput::HiHat {
            event: TriggerEvent::new(pad, trigger.velocity, TriggerSource::Midi, timestamp_us),
            articulation: trigger.articulation,
            sound: trigger.sound,
        });
    }

    fn remember_held(&mut self, channel: u8, note: u8, pad: PadId) {
        if let Some(entry) = self
            .held
            .iter_mut()
            .find(|(held_channel, held_note, _)| *held_channel == channel && *held_note == note)
        {
            entry.2 = pad;
        } else {
            self.held.push((channel, note, pad));
        }
    }

    /// Release every held note, e.g. on device disconnect or focus loss.
    pub fn release_all(&mut self, timestamp_us: u64) -> Vec<ReleaseEvent> {
        let mut releases = Vec::with_capacity(self.held.len());
        for (_, _, pad) in self.held.drain(..) {
            releases.push(ReleaseEvent::new(pad, TriggerSource::Midi, timestamp_us));
        }
        releases
    }

    /// Number of held notes, for diagnostics.
    #[must_use]
    pub fn held_count(&self) -> usize {
        self.held.len()
    }

    /// Forget parser state, e.g. after a reconnect.
    pub fn reset_parser(&mut self) {
        self.parser.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::monitor::MidiMonitor;
    use crate::trigger::PadIdExt;

    fn router() -> MidiRouter {
        MidiRouter::nitro_max()
    }

    #[test]
    fn note_on_triggers_the_mapped_pad() {
        let mut router = router();
        let mut out = Vec::new();
        router.process_bytes(&[0x90, 36, 100], 1_000, &mut out);
        assert_eq!(out.len(), 1);
        let MidiOutput::Trigger(event) = out[0] else {
            panic!("expected trigger, got {out:?}");
        };
        assert_eq!(event.pad, PadId::from_index(0));
        assert_eq!(event.velocity.get(), 100);
        assert_eq!(event.timestamp_us, 1_000);
        assert_eq!(router.held_count(), 1);
    }

    #[test]
    fn note_off_releases_the_same_pad() {
        let mut router = router();
        let mut out = Vec::new();
        router.process_bytes(&[0x90, 38, 100], 10, &mut out);
        out.clear();
        router.process_bytes(&[0x80, 38, 0], 20, &mut out);
        assert_eq!(
            out,
            vec![MidiOutput::Release(ReleaseEvent::new(
                PadId::from_index(1),
                TriggerSource::Midi,
                20
            ))]
        );
        assert_eq!(router.held_count(), 0);
    }

    #[test]
    fn note_on_velocity_zero_releases() {
        let mut router = router();
        let mut out = Vec::new();
        router.process_bytes(&[0x90, 36, 100], 1, &mut out);
        out.clear();
        router.process_bytes(&[0x90, 36, 0], 2, &mut out);
        assert!(matches!(out.as_slice(), [MidiOutput::Release(_)]));
        assert_eq!(router.held_count(), 0);
    }

    #[test]
    fn unmapped_notes_and_controllers_are_reported() {
        let mut router = router();
        let mut out = Vec::new();
        router.process_bytes(&[0x90, 1, 100], 1, &mut out);
        assert_eq!(
            out,
            vec![MidiOutput::Ignored(MidiIgnoreReason::UnmappedNote)]
        );
        out.clear();
        router.process_bytes(&[0xB0, 7, 100], 2, &mut out);
        assert_eq!(
            out,
            vec![MidiOutput::Ignored(MidiIgnoreReason::UnmappedControl)]
        );
    }

    #[test]
    fn channel_filter_rejects_other_channels() {
        let mut router = router();
        router.set_channel_filter(ChannelFilter::Single(9));
        let mut out = Vec::new();
        router.process_bytes(&[0x90, 36, 100], 1, &mut out);
        assert_eq!(
            out,
            vec![MidiOutput::Ignored(MidiIgnoreReason::ChannelFiltered)]
        );
        out.clear();
        router.process_bytes(&[0x99, 36, 100], 2, &mut out);
        assert!(matches!(out.as_slice(), [MidiOutput::Trigger(_)]));
    }

    #[test]
    fn hihat_notes_carry_articulation() {
        let mut router = router();
        let mut out = Vec::new();
        router.process_bytes(&[0x99, 46, 100], 1, &mut out);
        let MidiOutput::HiHat {
            event,
            articulation,
            sound,
        } = out[0]
        else {
            panic!("expected hi-hat, got {out:?}");
        };
        assert_eq!(event.pad, PadId::from_index(9));
        assert_eq!(articulation, HiHatArticulation::Open);
        assert_eq!(sound, HiHatSound::Hat);
        out.clear();
        router.process_bytes(&[0x99, 44, 80], 2, &mut out);
        let MidiOutput::HiHat {
            articulation,
            sound,
            ..
        } = out[0]
        else {
            panic!("expected hi-hat");
        };
        assert_eq!(articulation, HiHatArticulation::Pedal);
        assert_eq!(sound, HiHatSound::PedalChick);
    }

    #[test]
    fn cc4_produces_articulation_changes_on_the_hihat_pad() {
        let mut router = router();
        let mut out = Vec::new();
        router.process_bytes(&[0xB9, 4, 60], 1, &mut out);
        let MidiOutput::HiHat {
            event,
            articulation,
            ..
        } = out[0]
        else {
            panic!("expected hi-hat, got {out:?}");
        };
        assert_eq!(event.pad, PadId::from_index(9));
        assert_eq!(articulation, HiHatArticulation::HalfOpen);
    }

    #[test]
    fn learn_captures_without_triggering() {
        let mut router = router();
        router.learn_mut().arm(PadId::from_index(12));
        let mut out = Vec::new();
        router.process_bytes(&[0x90, 60, 90], 1, &mut out);
        assert_eq!(
            out,
            vec![MidiOutput::Learned {
                pad: PadId::from_index(12),
                control: CapturedControl::Note {
                    note: 60,
                    channel: 0
                }
            }]
        );
        assert_eq!(router.held_count(), 0);
        out.clear();
        router.learn_mut().arm(PadId::from_index(12));
        router.process_bytes(&[0xB9, 4, 64], 2, &mut out);
        assert!(matches!(out.as_slice(), [MidiOutput::Learned { .. }]));
    }

    #[test]
    fn disconnect_releases_every_held_note() {
        let mut router = router();
        let mut out = Vec::new();
        router.process_bytes(&[0x90, 36, 100, 38, 90], 1, &mut out);
        let releases = router.release_all(50);
        assert_eq!(releases.len(), 2);
        assert!(releases.iter().all(|release| release.timestamp_us == 50));
        assert_eq!(router.held_count(), 0);
    }

    #[test]
    fn monitor_records_every_parsed_message() {
        let mut router = router();
        let mut monitor = MidiMonitor::new(16);
        let mut out = Vec::new();
        router.process_bytes_monitored(
            &[0x90, 36, 100, 0xF8, 0x80, 36, 0],
            1,
            &mut monitor,
            &mut out,
        );
        assert_eq!(monitor.total_seen(), 3);
        assert!(monitor.entries().any(|entry| entry.text.contains("C2")));
        assert!(monitor.entries().any(|entry| entry.text.contains("clock")));
    }

    #[test]
    fn velocity_curve_applies_to_midi() {
        let config = MidiRouterConfig {
            velocity: MidiVelocityConfig::new(crate::midi::velocity::VelocityCurve::Fixed(
                Velocity::new(55).unwrap(),
            )),
            ..MidiRouterConfig::default()
        };
        let mut router = MidiRouter::new(MidiNoteMap::nitro_max_template(), config).unwrap();
        let mut out = Vec::new();
        router.process_bytes(&[0x90, 36, 127], 1, &mut out);
        let MidiOutput::Trigger(event) = out[0] else {
            panic!("expected trigger");
        };
        assert_eq!(event.velocity.get(), 55);
    }

    #[test]
    fn hihat_can_be_disabled() {
        let config = MidiRouterConfig {
            hihat_enabled: false,
            ..MidiRouterConfig::default()
        };
        let mut router = MidiRouter::new(MidiNoteMap::nitro_max_template(), config).unwrap();
        let mut out = Vec::new();
        router.process_bytes(&[0x99, 46, 100], 1, &mut out);
        assert!(matches!(out.as_slice(), [MidiOutput::Trigger(_)]));
    }
}
