//! Deterministic trigger sequences used by the offline renderer, the fake
//! MIDI/keyboard harness, and golden tests.
//!
//! A sequence is a plain-text file so a failing test can print it verbatim and
//! a reviewer can hand-edit a repro:
//!
//! ```text
//! # frame  action     pad  velocity
//! 0        note_on    0    100
//! 480      note_off   0
//! 720      note_on    1    127
//! 960      all_off
//! ```
//!
//! Frames are logical sample frames at the render sample rate; no wall clock
//! or randomness is involved. Events may share a frame and keep file order.

use std::error::Error;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

/// Number of performance pads in the engine.
pub const PAD_COUNT: u8 = 16;

/// One action in a scripted sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Start a pad with a MIDI-style velocity.
    NoteOn {
        /// Pad index in `0..PAD_COUNT`.
        pad: u8,
        /// Velocity in `1..=127`.
        velocity: u8,
    },
    /// Release a gated pad.
    NoteOff {
        /// Pad index in `0..PAD_COUNT`.
        pad: u8,
    },
    /// Release every gate voice.
    AllNotesOff,
    /// Stop every voice with a short fade.
    Panic,
}

impl Action {
    /// True when the action targets a pad.
    pub const fn pad(self) -> Option<u8> {
        match self {
            Self::NoteOn { pad, .. } | Self::NoteOff { pad } => Some(pad),
            Self::AllNotesOff | Self::Panic => None,
        }
    }
}

/// A timestamped action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceEvent {
    /// Logical frame at which the action should happen.
    pub at_frame: u64,
    /// Action to perform.
    pub action: Action,
}

impl SequenceEvent {
    /// The block-aligned frame at which the engine will actually apply the
    /// event.
    ///
    /// The engine timestamps input at the playhead of the next block, so an
    /// event between two block boundaries is applied at the start of the next
    /// block. This function is the documented quantization rule and lets tests
    /// assert the exact frame they should observe.
    pub const fn scheduled_frame(&self, block_frames: usize) -> u64 {
        if block_frames <= 1 {
            return self.at_frame;
        }
        let block = block_frames as u64;
        self.at_frame.div_ceil(block) * block
    }
}

/// A validated, ordered trigger sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sequence {
    events: Vec<SequenceEvent>,
    source_lines: Vec<usize>,
}

impl Sequence {
    /// Build a sequence from already validated events.
    ///
    /// Error line numbers refer to the 1-based event position because an
    /// in-memory sequence has no source file.
    pub fn from_events(events: Vec<SequenceEvent>) -> Result<Self, SequenceError> {
        let mut sequence = Self {
            source_lines: (1..=events.len()).collect(),
            events,
        };
        sequence.validate()?;
        Ok(sequence)
    }

    /// Parse a sequence from text.
    pub fn parse(text: &str) -> Result<Self, SequenceError> {
        let mut events = Vec::new();
        let mut source_lines = Vec::new();
        for (index, raw_line) in text.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let event = parse_line(line).map_err(|kind| SequenceError::InvalidLine {
                line: line_number,
                kind,
                text: raw_line.to_owned(),
            })?;
            events.push(event);
            source_lines.push(line_number);
        }
        let mut sequence = Self {
            events,
            source_lines,
        };
        sequence.validate()?;
        Ok(sequence)
    }

    /// Load a sequence from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, SequenceError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| SequenceError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&text)
    }

    fn validate(&mut self) -> Result<(), SequenceError> {
        // File order is preserved because it is the order a reviewer sees;
        // same-frame events keep their written order.
        if let Some(index) = self
            .events
            .windows(2)
            .position(|pair| pair[0].at_frame > pair[1].at_frame)
        {
            let line = self.source_lines.get(index + 1).copied().unwrap_or(0);
            return Err(SequenceError::Unsorted { line });
        }
        for (index, event) in self.events.iter().enumerate() {
            if let Action::NoteOn { pad, velocity } = event.action {
                if pad >= PAD_COUNT {
                    return Err(SequenceError::PadOutOfRange {
                        line: self.source_lines.get(index).copied().unwrap_or(0),
                        pad,
                    });
                }
                if velocity == 0 || velocity > 127 {
                    return Err(SequenceError::VelocityOutOfRange {
                        line: self.source_lines.get(index).copied().unwrap_or(0),
                        velocity,
                    });
                }
            }
            if let Action::NoteOff { pad } = event.action
                && pad >= PAD_COUNT
            {
                return Err(SequenceError::PadOutOfRange {
                    line: self.source_lines.get(index).copied().unwrap_or(0),
                    pad,
                });
            }
        }
        Ok(())
    }

    /// Events in application order.
    pub fn events(&self) -> &[SequenceEvent] {
        &self.events
    }

    /// Number of events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// True when the sequence has no events.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Last event's logical frame, or zero for an empty sequence.
    ///
    /// This is the time the event is written for. The engine applies it at the
    /// next block boundary, so render length must use
    /// [`Sequence::last_scheduled_frame`] instead.
    pub fn last_frame(&self) -> u64 {
        self.events.last().map(|event| event.at_frame).unwrap_or(0)
    }

    /// Frame at or after which the engine applies the last event.
    ///
    /// Use this when sizing a render: with 128-frame blocks, an event written
    /// at frame 1 is applied at frame 128, and a render sized to `last_frame`
    /// would drop it.
    pub fn last_scheduled_frame(&self, block_frames: usize) -> u64 {
        self.events
            .last()
            .map(|event| event.scheduled_frame(block_frames))
            .unwrap_or(0)
    }

    /// Render the sequence back to the canonical text form. Used by failure
    /// output and by `--print-sequence`.
    pub fn to_text(&self) -> String {
        let mut out = String::from("# frame action pad velocity\n");
        for event in &self.events {
            match event.action {
                Action::NoteOn { pad, velocity } => out.push_str(&format!(
                    "{} note_on {} {}\n",
                    event.at_frame, pad, velocity
                )),
                Action::NoteOff { pad } => {
                    out.push_str(&format!("{} note_off {}\n", event.at_frame, pad));
                }
                Action::AllNotesOff => {
                    out.push_str(&format!("{} all_off\n", event.at_frame));
                }
                Action::Panic => out.push_str(&format!("{} panic\n", event.at_frame)),
            }
        }
        out
    }

    /// Events that become due at `block_start`, in file order. An event is due
    /// when `at_frame <= block_start` and it has not been returned before.
    pub fn due<'a>(
        &'a self,
        cursor: &mut usize,
        block_start: u64,
    ) -> impl Iterator<Item = &'a SequenceEvent> + 'a {
        let start = *cursor;
        let mut end = start;
        while end < self.events.len() && self.events[end].at_frame <= block_start {
            end += 1;
        }
        *cursor = end;
        self.events[start..end].iter()
    }
}

fn parse_line(line: &str) -> Result<SequenceEvent, LineError> {
    let mut parts = line.split_whitespace();
    let frame_text = parts.next().ok_or(LineError::MissingFrame)?;
    let at_frame =
        u64::from_str(frame_text).map_err(|_| LineError::BadFrame(frame_text.to_owned()))?;
    let action_text = parts.next().ok_or(LineError::MissingAction)?;
    let action = match action_text {
        "note_on" | "trigger" => {
            let pad_text = parts.next().ok_or(LineError::MissingPad)?;
            let pad = u8::from_str(pad_text).map_err(|_| LineError::BadPad(pad_text.to_owned()))?;
            let velocity_text = parts.next().ok_or(LineError::MissingVelocity)?;
            let velocity = u8::from_str(velocity_text)
                .map_err(|_| LineError::BadVelocity(velocity_text.to_owned()))?;
            Action::NoteOn { pad, velocity }
        }
        "note_off" | "release" => {
            let pad_text = parts.next().ok_or(LineError::MissingPad)?;
            let pad = u8::from_str(pad_text).map_err(|_| LineError::BadPad(pad_text.to_owned()))?;
            Action::NoteOff { pad }
        }
        "all_off" | "all_notes_off" => Action::AllNotesOff,
        "panic" => Action::Panic,
        other => return Err(LineError::UnknownAction(other.to_owned())),
    };
    if let Some(extra) = parts.next() {
        return Err(LineError::TrailingToken(extra.to_owned()));
    }
    Ok(SequenceEvent { at_frame, action })
}

/// Why one line could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineError {
    /// The frame column is missing.
    MissingFrame,
    /// The frame is not an unsigned integer.
    BadFrame(String),
    /// The action column is missing.
    MissingAction,
    /// The action is not recognized.
    UnknownAction(String),
    /// The pad column is missing.
    MissingPad,
    /// The pad is not an unsigned integer.
    BadPad(String),
    /// The velocity column is missing.
    MissingVelocity,
    /// The velocity is not an unsigned integer.
    BadVelocity(String),
    /// An unexpected token follows the action.
    TrailingToken(String),
}

impl fmt::Display for LineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrame => write!(formatter, "missing frame column"),
            Self::BadFrame(text) => write!(formatter, "frame is not an integer: {text}"),
            Self::MissingAction => write!(formatter, "missing action column"),
            Self::UnknownAction(text) => write!(
                formatter,
                "unknown action {text}; expected note_on, note_off, all_off, or panic"
            ),
            Self::MissingPad => write!(formatter, "missing pad column"),
            Self::BadPad(text) => write!(formatter, "pad is not an integer: {text}"),
            Self::MissingVelocity => write!(formatter, "missing velocity column"),
            Self::BadVelocity(text) => write!(formatter, "velocity is not an integer: {text}"),
            Self::TrailingToken(text) => write!(formatter, "unexpected trailing token: {text}"),
        }
    }
}

/// Errors from sequence parsing and validation.
#[derive(Debug)]
pub enum SequenceError {
    /// A line could not be parsed.
    InvalidLine {
        /// 1-based line number.
        line: usize,
        /// Column-level reason.
        kind: LineError,
        /// The original line text.
        text: String,
    },
    /// Events are not in non-decreasing frame order.
    Unsorted {
        /// First offending line.
        line: usize,
    },
    /// A pad index is outside `0..PAD_COUNT`.
    PadOutOfRange {
        /// Line number.
        line: usize,
        /// Offending pad.
        pad: u8,
    },
    /// A velocity is outside `1..=127`.
    VelocityOutOfRange {
        /// Line number.
        line: usize,
        /// Offending velocity.
        velocity: u8,
    },
    /// Reading the file failed.
    Io {
        /// Path that could not be read.
        path: String,
        /// Underlying IO error.
        source: std::io::Error,
    },
}

impl fmt::Display for SequenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLine { line, kind, text } => {
                write!(formatter, "line {line}: {kind} in {text:?}")
            }
            Self::Unsorted { line } => {
                write!(
                    formatter,
                    "line {line}: events must be in non-decreasing frame order"
                )
            }
            Self::PadOutOfRange { line, pad } => write!(
                formatter,
                "line {line}: pad {pad} is outside 0..{}",
                PAD_COUNT
            ),
            Self::VelocityOutOfRange { line, velocity } => write!(
                formatter,
                "line {line}: velocity {velocity} is outside 1..=127"
            ),
            Self::Io { path, source } => write!(formatter, "cannot read {path}: {source}"),
        }
    }
}

impl Error for SequenceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# a comment
0 note_on 0 100
480 note_off 0
720 trigger 1 127
960 all_off
1200 panic
";

    #[test]
    fn parses_the_documented_format() {
        let sequence = Sequence::parse(SAMPLE).expect("parse");
        assert_eq!(sequence.len(), 5);
        assert_eq!(
            sequence.events()[0],
            SequenceEvent {
                at_frame: 0,
                action: Action::NoteOn {
                    pad: 0,
                    velocity: 100
                }
            }
        );
        assert_eq!(sequence.events()[1].action, Action::NoteOff { pad: 0 });
    }

    #[test]
    fn round_trips_through_text() {
        let sequence = Sequence::parse(SAMPLE).expect("parse");
        let reparsed = Sequence::parse(&sequence.to_text()).expect("reparse");
        assert_eq!(sequence.events(), reparsed.events());
    }

    #[test]
    fn rejects_bad_lines_with_line_numbers() {
        let error = Sequence::parse("0 note_on 0 100\n2 note_on 99 100\n").expect_err("error");
        assert!(
            matches!(error, SequenceError::PadOutOfRange { line: 2, .. }),
            "{error}"
        );

        let error = Sequence::parse("0 note_on 0 0\n").expect_err("error");
        assert!(matches!(
            error,
            SequenceError::VelocityOutOfRange {
                line: 1,
                velocity: 0
            }
        ));

        let error = Sequence::parse("zero note_on 0 100\n").expect_err("error");
        assert!(matches!(
            error,
            SequenceError::InvalidLine {
                line: 1,
                kind: LineError::BadFrame(_),
                ..
            }
        ));
    }

    #[test]
    fn rejects_out_of_order_events() {
        let error = Sequence::parse("480 note_on 0 100\n0 note_off 0\n").expect_err("error");
        assert!(
            matches!(error, SequenceError::Unsorted { line: 2 }),
            "{error}"
        );
    }

    #[test]
    fn block_quantization_rounds_up_to_the_next_block() {
        let event = SequenceEvent {
            at_frame: 100,
            action: Action::AllNotesOff,
        };
        assert_eq!(event.scheduled_frame(128), 128);
        assert_eq!(event.scheduled_frame(1), 100);
        assert_eq!(
            SequenceEvent {
                at_frame: 128,
                action: Action::AllNotesOff
            }
            .scheduled_frame(128),
            128
        );
        assert_eq!(
            SequenceEvent {
                at_frame: 0,
                action: Action::AllNotesOff
            }
            .scheduled_frame(128),
            0
        );
    }

    #[test]
    fn due_returns_events_once_in_file_order() {
        let sequence = Sequence::parse(SAMPLE).expect("parse");
        let mut cursor = 0;
        let first: Vec<_> = sequence.due(&mut cursor, 0).copied().collect();
        assert_eq!(first.len(), 1);
        let second: Vec<_> = sequence.due(&mut cursor, 500).copied().collect();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].at_frame, 480);
        let rest: Vec<_> = sequence.due(&mut cursor, 2000).copied().collect();
        assert_eq!(rest.len(), 3);
        assert!(sequence.due(&mut cursor, 3000).next().is_none());
    }

    #[test]
    fn same_frame_events_keep_file_order() {
        let sequence = Sequence::parse("0 all_off\n0 note_on 2 64\n").expect("parse");
        assert_eq!(
            sequence.events()[0].action,
            Action::AllNotesOff,
            "stable order must be preserved"
        );
    }
}
