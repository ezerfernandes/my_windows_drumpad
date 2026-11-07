//! MIDI byte-stream parsing.
//!
//! The parser is incremental: the backend hands it chunks of bytes from the
//! driver callback and it emits complete channel messages. It supports running
//! status, treats note-on velocity zero as note-off, ignores system realtime
//! bytes (clock, active sensing, reset) wherever they appear, and classifies
//! everything else as an ignored message rather than guessing.
//!
//! Parsing is allocation free. Callers pass a reusable `Vec` for output; the
//! MIDI callback can keep one per connection.

use serde::{Deserialize, Serialize};

use crate::trigger::{Velocity, VelocityExt};

/// Channel filter applied before mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "mode", content = "channels")]
pub enum ChannelFilter {
    /// Accept every channel.
    #[default]
    All,
    /// Accept one zero-based channel (`0..=15`).
    Single(u8),
    /// Accept the listed zero-based channels.
    Multiple(Vec<u8>),
}

impl ChannelFilter {
    /// Build a single-channel filter from a human one-based channel number.
    #[must_use]
    pub fn from_one_based(channel: u8) -> Option<Self> {
        channel
            .checked_sub(1)
            .filter(|value| *value < 16)
            .map(Self::Single)
    }

    /// True when `channel` (zero-based) passes.
    #[must_use]
    pub fn accepts(&self, channel: u8) -> bool {
        match self {
            Self::All => true,
            Self::Single(expected) => *expected == channel,
            Self::Multiple(channels) => channels.contains(&channel),
        }
    }

    /// True when every entry is a valid zero-based channel.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        match self {
            Self::All => true,
            Self::Single(channel) => *channel < 16,
            Self::Multiple(channels) => channels.iter().all(|channel| *channel < 16),
        }
    }
}

/// Why a message was not useful to the trigger path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreKind {
    /// MIDI clock (0xF8).
    Clock,
    /// Active sensing (0xFE).
    ActiveSensing,
    /// System reset (0xFF).
    SystemReset,
    /// SysEx or another system-common message.
    System,
    /// A channel message the trigger path does not use, such as aftertouch.
    Unsupported,
    /// A data byte arrived with no status byte.
    Malformed,
}

/// A parsed MIDI message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiEvent {
    /// Note on with a non-zero velocity.
    NoteOn {
        /// Zero-based channel.
        channel: u8,
        /// Note number `0..=127`.
        note: u8,
        /// Velocity `1..=127`.
        velocity: Velocity,
    },
    /// Note off, including note-on with velocity zero.
    NoteOff {
        /// Zero-based channel.
        channel: u8,
        /// Note number `0..=127`.
        note: u8,
    },
    /// Controller change.
    ControlChange {
        /// Zero-based channel.
        channel: u8,
        /// Controller number `0..=127`.
        controller: u8,
        /// Controller value `0..=127`.
        value: u8,
    },
    /// A message the trigger path deliberately ignores.
    Ignored {
        /// Raw status byte, when one was involved.
        status: u8,
        /// Why it was ignored.
        kind: IgnoreKind,
    },
}

impl MidiEvent {
    /// Zero-based channel, when the message has one.
    #[must_use]
    pub const fn channel(self) -> Option<u8> {
        match self {
            Self::NoteOn { channel, .. }
            | Self::NoteOff { channel, .. }
            | Self::ControlChange { channel, .. } => Some(channel),
            Self::Ignored { .. } => None,
        }
    }
}

/// Incremental MIDI stream parser.
#[derive(Debug, Clone, Default)]
pub struct MidiParser {
    running_status: Option<u8>,
    data: [u8; 2],
    data_len: usize,
    needed: usize,
}

impl MidiParser {
    /// Fresh parser with no running status.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget partial state, e.g. after a reconnect.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feed one byte. Appends zero or one event to `out`.
    pub fn push(&mut self, byte: u8, out: &mut Vec<MidiEvent>) {
        // System realtime bytes may appear between any two bytes of another
        // message and never disturb running status.
        if byte >= 0xF8 {
            let kind = match byte {
                0xF8 => IgnoreKind::Clock,
                0xFE => IgnoreKind::ActiveSensing,
                0xFF => IgnoreKind::SystemReset,
                _ => IgnoreKind::System,
            };
            out.push(MidiEvent::Ignored { status: byte, kind });
            return;
        }

        if byte & 0x80 != 0 {
            if byte >= 0xF0 {
                self.running_status = None;
                self.data_len = 0;
                self.needed = 0;
                out.push(MidiEvent::Ignored {
                    status: byte,
                    kind: IgnoreKind::System,
                });
                return;
            }
            self.running_status = Some(byte);
            self.data_len = 0;
            self.needed = data_bytes_for(byte);
            return;
        }

        let Some(status) = self.running_status else {
            out.push(MidiEvent::Ignored {
                status: 0,
                kind: IgnoreKind::Malformed,
            });
            return;
        };

        self.data[self.data_len] = byte;
        self.data_len += 1;
        if self.data_len < self.needed {
            return;
        }

        let event = decode(status, self.data);
        self.data_len = 0;
        out.push(event);
    }

    /// Feed a chunk, appending every completed event.
    pub fn push_slice(&mut self, bytes: &[u8], out: &mut Vec<MidiEvent>) {
        for byte in bytes {
            self.push(*byte, out);
        }
    }

    /// Convenience for tests and fixtures: parse a complete byte slice.
    #[must_use]
    pub fn parse_all(bytes: &[u8]) -> Vec<MidiEvent> {
        let mut parser = Self::new();
        let mut events = Vec::new();
        parser.push_slice(bytes, &mut events);
        events
    }
}

fn data_bytes_for(status: u8) -> usize {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        _ => 2,
    }
}

fn decode(status: u8, data: [u8; 2]) -> MidiEvent {
    let channel = status & 0x0F;
    match status & 0xF0 {
        0x80 => MidiEvent::NoteOff {
            channel,
            note: data[0],
        },
        0x90 => match Velocity::from_midi(data[1]) {
            Some(velocity) => MidiEvent::NoteOn {
                channel,
                note: data[0],
                velocity,
            },
            None => MidiEvent::NoteOff {
                channel,
                note: data[0],
            },
        },
        0xB0 => MidiEvent::ControlChange {
            channel,
            controller: data[0],
            value: data[1],
        },
        _ => MidiEvent::Ignored {
            status,
            kind: IgnoreKind::Unsupported,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn velocity(value: u8) -> Velocity {
        Velocity::new(value).unwrap()
    }

    #[test]
    fn note_on_and_off_parse() {
        assert_eq!(
            MidiParser::parse_all(&[0x90, 36, 100]),
            vec![MidiEvent::NoteOn {
                channel: 0,
                note: 36,
                velocity: velocity(100)
            }]
        );
        assert_eq!(
            MidiParser::parse_all(&[0x80, 38, 64]),
            vec![MidiEvent::NoteOff {
                channel: 0,
                note: 38
            }]
        );
    }

    #[test]
    fn note_on_velocity_zero_is_note_off() {
        assert_eq!(
            MidiParser::parse_all(&[0x95, 36, 0]),
            vec![MidiEvent::NoteOff {
                channel: 5,
                note: 36
            }]
        );
    }

    #[test]
    fn control_changes_parse() {
        assert_eq!(
            MidiParser::parse_all(&[0xB9, 4, 64, 0xB9, 64, 127]),
            vec![
                MidiEvent::ControlChange {
                    channel: 9,
                    controller: 4,
                    value: 64
                },
                MidiEvent::ControlChange {
                    channel: 9,
                    controller: 64,
                    value: 127
                },
            ]
        );
    }

    #[test]
    fn running_status_is_honoured() {
        assert_eq!(
            MidiParser::parse_all(&[0x90, 36, 100, 38, 90, 40, 80]),
            vec![
                MidiEvent::NoteOn {
                    channel: 0,
                    note: 36,
                    velocity: velocity(100)
                },
                MidiEvent::NoteOn {
                    channel: 0,
                    note: 38,
                    velocity: velocity(90)
                },
                MidiEvent::NoteOn {
                    channel: 0,
                    note: 40,
                    velocity: velocity(80)
                },
            ]
        );
    }

    #[test]
    fn realtime_bytes_do_not_break_running_status() {
        let events = MidiParser::parse_all(&[0x90, 36, 0xF8, 100, 38, 90, 0xFE]);
        assert_eq!(events.len(), 4);
        assert_eq!(
            events[0],
            MidiEvent::Ignored {
                status: 0xF8,
                kind: IgnoreKind::Clock
            }
        );
        assert_eq!(
            events[1],
            MidiEvent::NoteOn {
                channel: 0,
                note: 36,
                velocity: velocity(100)
            }
        );
        assert_eq!(
            events[2],
            MidiEvent::NoteOn {
                channel: 0,
                note: 38,
                velocity: velocity(90)
            }
        );
        assert_eq!(
            events[3],
            MidiEvent::Ignored {
                status: 0xFE,
                kind: IgnoreKind::ActiveSensing
            }
        );
    }

    #[test]
    fn system_messages_clear_running_status() {
        let events = MidiParser::parse_all(&[0x90, 36, 100, 0xF0, 0x01, 0x02, 0xF7, 36, 100]);
        assert!(events.iter().any(|event| matches!(
            event,
            MidiEvent::Ignored {
                status: 0xF0,
                kind: IgnoreKind::System
            }
        )));
        // After SysEx, the bare data bytes have no status and are malformed.
        assert!(events.iter().any(|event| matches!(
            event,
            MidiEvent::Ignored {
                kind: IgnoreKind::Malformed,
                ..
            }
        )));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, MidiEvent::NoteOn { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn stray_data_bytes_are_malformed() {
        assert_eq!(
            MidiParser::parse_all(&[36, 100]),
            vec![
                MidiEvent::Ignored {
                    status: 0,
                    kind: IgnoreKind::Malformed
                },
                MidiEvent::Ignored {
                    status: 0,
                    kind: IgnoreKind::Malformed
                }
            ]
        );
    }

    #[test]
    fn program_change_consumes_one_data_byte() {
        let mut parser = MidiParser::new();
        let mut events = Vec::new();
        parser.push_slice(&[0xC0, 5, 0x90, 36, 100], &mut events);
        assert_eq!(
            events,
            vec![
                MidiEvent::Ignored {
                    status: 0xC0,
                    kind: IgnoreKind::Unsupported
                },
                MidiEvent::NoteOn {
                    channel: 0,
                    note: 36,
                    velocity: velocity(100)
                }
            ]
        );
    }

    #[test]
    fn channel_filter_matches_documentation() {
        assert!(ChannelFilter::All.accepts(15));
        assert!(ChannelFilter::Single(9).accepts(9));
        assert!(!ChannelFilter::Single(9).accepts(0));
        assert!(ChannelFilter::Multiple(vec![0, 9]).accepts(9));
        assert!(!ChannelFilter::Multiple(vec![0, 9]).accepts(3));
        assert_eq!(
            ChannelFilter::from_one_based(10),
            Some(ChannelFilter::Single(9))
        );
        assert_eq!(ChannelFilter::from_one_based(0), None);
        assert!(!ChannelFilter::Single(16).is_valid());
        assert!(ChannelFilter::default().is_valid());
    }

    #[test]
    fn split_messages_parse_across_chunks() {
        let mut parser = MidiParser::new();
        let mut events = Vec::new();
        parser.push_slice(&[0x90, 36], &mut events);
        assert!(events.is_empty());
        parser.push_slice(&[100], &mut events);
        assert_eq!(
            events,
            vec![MidiEvent::NoteOn {
                channel: 0,
                note: 36,
                velocity: velocity(100)
            }]
        );
    }

    #[test]
    fn reset_clears_partial_state() {
        let mut parser = MidiParser::new();
        let mut events = Vec::new();
        parser.push_slice(&[0x90, 36], &mut events);
        parser.reset();
        parser.push(100, &mut events);
        assert!(matches!(
            events.last(),
            Some(MidiEvent::Ignored {
                kind: IgnoreKind::Malformed,
                ..
            })
        ));
    }
}
