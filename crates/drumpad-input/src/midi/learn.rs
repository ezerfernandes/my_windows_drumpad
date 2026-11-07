//! MIDI learn.
//!
//! The app arms a pad, the user hits a pad or moves a pedal, and the next note
//! or controller message is captured for that pad. Learn captures notes and CC
//! messages; the caller applies the capture to a [`MidiNoteMap`] with an
//! explicit conflict policy so nothing is overwritten silently.

use crate::midi::note_map::{AssignPolicy, MidiNoteMap, NoteConflict, NoteMapping};
use crate::midi::parse::MidiEvent;
use crate::trigger::PadId;

/// A captured MIDI control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturedControl {
    /// A note number.
    Note {
        /// Note that was hit.
        note: u8,
        /// Zero-based channel.
        channel: u8,
    },
    /// A continuous controller.
    ControlChange {
        /// Controller number.
        controller: u8,
        /// Zero-based channel.
        channel: u8,
    },
}

impl CapturedControl {
    /// Human-readable description for the inspector.
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::Note { note, channel } => {
                format!(
                    "note {} on channel {}",
                    super::monitor::note_name(note),
                    channel + 1
                )
            }
            Self::ControlChange {
                controller,
                channel,
            } => {
                format!("CC{} on channel {}", controller, channel + 1)
            }
        }
    }
}

/// Result of observing one message while armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnOutcome {
    /// A control was captured for the armed pad.
    Captured {
        /// Pad being learned.
        pad: PadId,
        /// Captured control.
        control: CapturedControl,
    },
    /// No pad is armed.
    NotArmed,
    /// The message is not learnable (note off, clock, unsupported).
    Ignored,
}

/// Learn state machine.
#[derive(Debug, Clone, Default)]
pub struct MidiLearn {
    armed: Option<PadId>,
    last: Option<LearnOutcome>,
}

impl MidiLearn {
    /// Create an idle learn session.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Arm `pad` for the next learnable message.
    pub fn arm(&mut self, pad: PadId) {
        self.armed = Some(pad);
        self.last = None;
    }

    /// Cancel learn.
    pub fn disarm(&mut self) {
        self.armed = None;
    }

    /// Pad waiting for a message, if any.
    #[must_use]
    pub fn armed(&self) -> Option<PadId> {
        self.armed
    }

    /// Result of the most recent observation, for the inspector.
    #[must_use]
    pub fn last_outcome(&self) -> Option<LearnOutcome> {
        self.last
    }

    /// Observe a parsed message.
    pub fn observe(&mut self, event: MidiEvent) -> LearnOutcome {
        let Some(pad) = self.armed else {
            return LearnOutcome::NotArmed;
        };
        let control = match event {
            MidiEvent::NoteOn { channel, note, .. } => CapturedControl::Note { note, channel },
            MidiEvent::ControlChange {
                channel,
                controller,
                ..
            } => CapturedControl::ControlChange {
                controller,
                channel,
            },
            MidiEvent::NoteOff { .. } | MidiEvent::Ignored { .. } => {
                return LearnOutcome::Ignored;
            }
        };
        let outcome = LearnOutcome::Captured { pad, control };
        self.last = Some(outcome);
        self.armed = None;
        outcome
    }
}

/// Apply a learned note to a map.
pub fn apply_note(
    map: &mut MidiNoteMap,
    note: u8,
    pad: PadId,
    policy: AssignPolicy,
) -> Result<Option<NoteMapping>, NoteConflict> {
    map.assign(note, pad, format!("Learned note {note}"), policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::note_map::AssignPolicy;
    use crate::trigger::PadIdExt;
    use crate::trigger::Velocity;

    fn note_on(note: u8) -> MidiEvent {
        MidiEvent::NoteOn {
            channel: 0,
            note,
            velocity: Velocity::new(100).unwrap(),
        }
    }

    #[test]
    fn captures_the_next_note() {
        let mut learn = MidiLearn::new();
        let pad = PadId::from_index(4);
        assert_eq!(learn.observe(note_on(40)), LearnOutcome::NotArmed);
        learn.arm(pad);
        let outcome = learn.observe(note_on(40));
        assert_eq!(
            outcome,
            LearnOutcome::Captured {
                pad,
                control: CapturedControl::Note {
                    note: 40,
                    channel: 0
                }
            }
        );
        assert_eq!(learn.armed(), None);
        assert_eq!(learn.last_outcome(), Some(outcome));
        assert_eq!(learn.observe(note_on(41)), LearnOutcome::NotArmed);
    }

    #[test]
    fn captures_control_changes() {
        let mut learn = MidiLearn::new();
        learn.arm(PadId::from_index(2));
        let outcome = learn.observe(MidiEvent::ControlChange {
            channel: 9,
            controller: 4,
            value: 64,
        });
        assert_eq!(
            outcome,
            LearnOutcome::Captured {
                pad: PadId::from_index(2),
                control: CapturedControl::ControlChange {
                    controller: 4,
                    channel: 9
                }
            }
        );
    }

    #[test]
    fn note_off_and_ignored_messages_do_not_capture() {
        let mut learn = MidiLearn::new();
        learn.arm(PadId::from_index(0));
        assert_eq!(
            learn.observe(MidiEvent::NoteOff {
                channel: 0,
                note: 36
            }),
            LearnOutcome::Ignored
        );
        assert_eq!(learn.armed(), Some(PadId::from_index(0)));
        assert!(learn.last_outcome().is_none());
    }

    #[test]
    fn applying_a_captured_note_respects_conflicts() {
        let mut map = MidiNoteMap::nitro_max_template();
        let conflict =
            apply_note(&mut map, 36, PadId::from_index(12), AssignPolicy::Reject).unwrap_err();
        assert_eq!(conflict.existing_pad, PadId::from_index(0));
        apply_note(&mut map, 36, PadId::from_index(12), AssignPolicy::Replace).unwrap();
        assert_eq!(map.pad_for(36), Some(PadId::from_index(12)));
    }

    #[test]
    fn captured_control_renders_for_the_inspector() {
        assert_eq!(
            CapturedControl::Note {
                note: 60,
                channel: 9
            }
            .describe(),
            "note C4 on channel 10"
        );
        assert_eq!(
            CapturedControl::ControlChange {
                controller: 64,
                channel: 0
            }
            .describe(),
            "CC64 on channel 1"
        );
    }
}
