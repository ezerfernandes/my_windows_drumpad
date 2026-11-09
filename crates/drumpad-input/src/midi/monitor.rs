//! In-memory MIDI monitor.
//!
//! The monitor is a bounded ring of formatted entries that the Settings view
//! polls once per frame. It is fed from the MIDI callback, never from the audio
//! thread. Entries carry a formatted `String`, so the callback does allocate
//! here; keep the ring small and pause it when the Settings view is hidden.
//! The app can pause and clear the view without disturbing capture.

use std::collections::VecDeque;

use crate::midi::parse::{IgnoreKind, MidiEvent};

/// One line in the monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiMonitorEntry {
    /// Driver timestamp in microseconds, as delivered by midir.
    pub timestamp_us: u64,
    /// Zero-based channel, when the message has one.
    pub channel: Option<u8>,
    /// Message category.
    pub kind: MonitorKind,
    /// Note number, for note messages.
    pub note: Option<u8>,
    /// Controller number, for CC messages.
    pub controller: Option<u8>,
    /// Velocity or controller value.
    pub value: Option<u8>,
    /// Preformatted display text, e.g. `"ch10 note C2 vel 100"`.
    pub text: String,
}

/// Message category shown in the monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorKind {
    /// Note on.
    NoteOn,
    /// Note off.
    NoteOff,
    /// Control change.
    ControlChange,
    /// Ignored or unsupported message.
    Ignored,
}

impl MonitorKind {
    /// Short label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoteOn => "note on",
            Self::NoteOff => "note off",
            Self::ControlChange => "cc",
            Self::Ignored => "ignored",
        }
    }
}

/// Bounded, pausable monitor.
#[derive(Debug)]
pub struct MidiMonitor {
    entries: VecDeque<MidiMonitorEntry>,
    capacity: usize,
    paused: bool,
    total_seen: u64,
    dropped: u64,
}

impl MidiMonitor {
    /// Monitor retaining the newest `capacity` entries.
    ///
    /// # Panics
    ///
    /// Panics when `capacity` is zero; a monitor with no retention is a
    /// configuration mistake, not a silent no-op.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "monitor capacity must be positive");
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
            paused: false,
            total_seen: 0,
            dropped: 0,
        }
    }

    /// Record a parsed message at `timestamp_us`.
    pub fn record(&mut self, timestamp_us: u64, event: MidiEvent) {
        self.total_seen += 1;
        if self.paused {
            return;
        }
        let entry = format_entry(timestamp_us, event);
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
            self.dropped += 1;
        }
        self.entries.push_back(entry);
    }

    /// True when recording is paused; the UI can still clear the view.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Pause or resume recording.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Drop every visible entry.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.dropped = 0;
    }

    /// Visible entries, oldest first.
    #[must_use]
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &MidiMonitorEntry> {
        self.entries.iter()
    }

    /// Total messages seen, including while paused and those evicted.
    #[must_use]
    pub fn total_seen(&self) -> u64 {
        self.total_seen
    }

    /// Entries evicted because the ring was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Number of visible entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the view is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn format_entry(timestamp_us: u64, event: MidiEvent) -> MidiMonitorEntry {
    match event {
        MidiEvent::NoteOn {
            channel,
            note,
            velocity,
        } => MidiMonitorEntry {
            timestamp_us,
            channel: Some(channel),
            kind: MonitorKind::NoteOn,
            note: Some(note),
            controller: None,
            value: Some(velocity.get()),
            text: format!(
                "ch{} note {} vel {}",
                channel + 1,
                note_name(note),
                velocity.get()
            ),
        },
        MidiEvent::NoteOff { channel, note } => MidiMonitorEntry {
            timestamp_us,
            channel: Some(channel),
            kind: MonitorKind::NoteOff,
            note: Some(note),
            controller: None,
            value: None,
            text: format!("ch{} note {} off", channel + 1, note_name(note)),
        },
        MidiEvent::ControlChange {
            channel,
            controller,
            value,
        } => MidiMonitorEntry {
            timestamp_us,
            channel: Some(channel),
            kind: MonitorKind::ControlChange,
            note: None,
            controller: Some(controller),
            value: Some(value),
            text: format!("ch{} cc{} = {}", channel + 1, controller, value),
        },
        MidiEvent::Ignored { status, kind } => MidiMonitorEntry {
            timestamp_us,
            channel: None,
            kind: MonitorKind::Ignored,
            note: None,
            controller: None,
            value: None,
            text: format!("ignored 0x{status:02X} ({})", ignore_label(kind)),
        },
    }
}

fn ignore_label(kind: IgnoreKind) -> &'static str {
    match kind {
        IgnoreKind::Clock => "clock",
        IgnoreKind::ActiveSensing => "active sensing",
        IgnoreKind::SystemReset => "system reset",
        IgnoreKind::System => "system",
        IgnoreKind::Unsupported => "unsupported",
        IgnoreKind::Malformed => "malformed",
    }
}

/// Scientific pitch notation for a MIDI note (`60` is `C4`).
#[must_use]
pub fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = i16::from(note) / 12 - 1;
    format!("{}{}", NAMES[usize::from(note % 12)], octave)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::Velocity;

    fn note_on(note: u8, value: u8) -> MidiEvent {
        MidiEvent::NoteOn {
            channel: 9,
            note,
            velocity: Velocity::new(value).unwrap(),
        }
    }

    #[test]
    fn records_notes_and_cc_with_accurate_text() {
        let mut monitor = MidiMonitor::new(8);
        monitor.record(100, note_on(36, 100));
        monitor.record(
            200,
            MidiEvent::ControlChange {
                channel: 9,
                controller: 4,
                value: 64,
            },
        );
        let entries: Vec<&MidiMonitorEntry> = monitor.entries().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "ch10 note C2 vel 100");
        assert_eq!(entries[0].kind, MonitorKind::NoteOn);
        assert_eq!(entries[0].value, Some(100));
        assert_eq!(entries[1].text, "ch10 cc4 = 64");
        assert_eq!(monitor.total_seen(), 2);
    }

    #[test]
    fn pause_stops_entries_but_keeps_counting() {
        let mut monitor = MidiMonitor::new(8);
        monitor.record(1, note_on(36, 100));
        monitor.set_paused(true);
        monitor.record(2, note_on(38, 90));
        assert_eq!(monitor.len(), 1);
        assert_eq!(monitor.total_seen(), 2);
        assert!(monitor.is_paused());
        monitor.set_paused(false);
        monitor.record(3, note_on(40, 80));
        assert_eq!(monitor.len(), 2);
    }

    #[test]
    fn clear_empties_the_view() {
        let mut monitor = MidiMonitor::new(8);
        monitor.record(1, note_on(36, 100));
        monitor.clear();
        assert!(monitor.is_empty());
        assert_eq!(monitor.total_seen(), 1);
    }

    #[test]
    fn capacity_is_bounded_and_drops_oldest() {
        let mut monitor = MidiMonitor::new(2);
        for index in 0..5 {
            monitor.record(index, note_on(36 + index as u8, 100));
        }
        assert_eq!(monitor.len(), 2);
        assert_eq!(monitor.dropped(), 3);
        let notes: Vec<Option<u8>> = monitor.entries().map(|entry| entry.note).collect();
        assert_eq!(notes, vec![Some(39), Some(40)]);
    }

    #[test]
    fn ignored_messages_are_explained() {
        let mut monitor = MidiMonitor::new(4);
        monitor.record(
            1,
            MidiEvent::Ignored {
                status: 0xF8,
                kind: IgnoreKind::Clock,
            },
        );
        let entry = monitor.entries().next().unwrap();
        assert_eq!(entry.text, "ignored 0xF8 (clock)");
        assert_eq!(entry.kind, MonitorKind::Ignored);
        assert_eq!(monitor.total_seen(), 1);
    }

    #[test]
    #[should_panic(expected = "capacity must be positive")]
    fn zero_capacity_is_rejected() {
        let _ = MidiMonitor::new(0);
    }

    #[test]
    fn note_names_use_scientific_pitch() {
        assert_eq!(note_name(0), "C-1");
        assert_eq!(note_name(21), "A0");
        assert_eq!(note_name(36), "C2");
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(127), "G9");
    }

    #[test]
    fn message_kinds_expose_short_labels() {
        assert_eq!(MonitorKind::NoteOn.label(), "note on");
        assert_eq!(MonitorKind::NoteOff.label(), "note off");
        assert_eq!(MonitorKind::ControlChange.label(), "cc");
        assert_eq!(MonitorKind::Ignored.label(), "ignored");
    }

    #[test]
    fn ignored_messages_name_every_reason() {
        let mut monitor = MidiMonitor::new(8);
        let expected = [
            (0xF8, IgnoreKind::Clock, "clock"),
            (0xFE, IgnoreKind::ActiveSensing, "active sensing"),
            (0xFF, IgnoreKind::SystemReset, "system reset"),
            (0xF0, IgnoreKind::System, "system"),
            (0xA0, IgnoreKind::Unsupported, "unsupported"),
            (0x00, IgnoreKind::Malformed, "malformed"),
        ];
        for (status, kind, label) in expected {
            monitor.record(1, MidiEvent::Ignored { status, kind });
            let entry = monitor.entries().last().unwrap();
            assert!(
                entry.text.contains(label),
                "{} did not mention {label}",
                entry.text
            );
        }
        assert_eq!(monitor.total_seen(), 6);
    }
}
