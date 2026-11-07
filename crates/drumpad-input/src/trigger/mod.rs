//! Source-agnostic input vocabulary.
//!
//! Keyboard and MIDI capture both produce [`TriggerEvent`], [`ReleaseEvent`]
//! and the transport actions in this module, so the engine, the UI and the
//! persistence layer never branch on where a hit came from.
//!
//! The types mirror the contract in `docs/architecture/interfaces.md`:
//!
//! ```text
//! TriggerEvent { pad: PadId, velocity: Velocity, source: TriggerSource, timestamp_us: u64 }
//! ReleaseEvent { pad: PadId, source: TriggerSource, timestamp_us: u64 }
//! ```
//!
//! `PadId` and `Velocity` are re-exported from `drumpad-model` so the input,
//! model, and engine crates share one permanent identity. The extension traits
//! below add only the index and MIDI helpers the capture paths need.
//!
//! Events are pushed into the lock-free single-producer/single-consumer queue
//! in [`queue`], which the audio callback drains. Nothing in this module
//! allocates, locks, logs or performs I/O on the push path.

pub mod clock;
pub mod latency;
pub mod queue;

use serde::{Deserialize, Serialize};

pub use clock::{EventClock, ManualClock, MonotonicClock};
pub use drumpad_model::{PadId, Velocity};
pub use latency::{LatencyPercentiles, LatencyProbe, LatencyRecorder};
pub use queue::{EventConsumer, EventProducer, PushError, event_queue};

/// Number of performance pads in the v1 4x4 grid.
pub const PAD_COUNT: usize = drumpad_model::ids::PAD_COUNT;

/// One-based human label for a pad, for example `"pad 1"`.
///
/// The model's `Display` is the raw zero-based wire value; user-facing strings
/// in profiles and the editor should use this helper.
#[must_use]
pub fn pad_label(pad: PadId) -> String {
    format!("pad {}", pad.get() + 1)
}

/// Softest MIDI velocity as a [`Velocity`].
pub const MIN_VELOCITY: Velocity = Velocity(Velocity::MIN);

/// Loudest MIDI velocity as a [`Velocity`].
pub const MAX_VELOCITY: Velocity = Velocity::MAXIMUM;

/// Capture-side helpers for the shared [`PadId`].
///
/// `drumpad-model` owns the permanent type; these helpers keep grid code
/// readable without defining a second pad identity.
pub trait PadIdExt {
    /// Every pad in grid order.
    const ALL: [PadId; PAD_COUNT];

    /// Build a pad from a zero-based index.
    ///
    /// # Panics
    ///
    /// Panics when `index >= PAD_COUNT`. Use `PadId::new` for untrusted input.
    #[must_use]
    fn from_index(index: u8) -> Self;
}

impl PadIdExt for PadId {
    const ALL: [PadId; PAD_COUNT] = [
        PadId(0),
        PadId(1),
        PadId(2),
        PadId(3),
        PadId(4),
        PadId(5),
        PadId(6),
        PadId(7),
        PadId(8),
        PadId(9),
        PadId(10),
        PadId(11),
        PadId(12),
        PadId(13),
        PadId(14),
        PadId(15),
    ];

    fn from_index(index: u8) -> Self {
        assert!((index as usize) < PAD_COUNT, "pad index out of range");
        PadId(index)
    }
}

/// Capture-side helpers for the shared [`Velocity`].
pub trait VelocityExt: Sized {
    /// Convert a raw MIDI velocity, returning `None` for `0` (note-off).
    #[must_use]
    fn from_midi(value: u8) -> Option<Self>;

    /// Build a velocity, clamping into `1..=127`.
    #[must_use]
    fn clamp_signed(value: i32) -> Self;
}

impl VelocityExt for Velocity {
    fn from_midi(value: u8) -> Option<Self> {
        Self::new(value).ok()
    }

    fn clamp_signed(value: i32) -> Self {
        if value < i32::from(Self::MIN) {
            MIN_VELOCITY
        } else if value > i32::from(Self::MAX) {
            MAX_VELOCITY
        } else {
            Self::from_raw_unchecked(value as u8)
        }
    }
}

/// Where a hit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSource {
    /// A physical keyboard or one-hand keypad.
    Keyboard,
    /// A MIDI note or pedal message.
    Midi,
    /// A click or test trigger from the UI.
    UiPreview,
}

impl TriggerSource {
    /// Stable lowercase identifier used in diagnostics and profiles.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard",
            Self::Midi => "midi",
            Self::UiPreview => "ui_preview",
        }
    }
}

/// A pad was struck.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerEvent {
    /// Pad that was struck.
    pub pad: PadId,
    /// Strike velocity in `1..=127`.
    pub velocity: Velocity,
    /// Capture path that produced the event.
    pub source: TriggerSource,
    /// Monotonic capture timestamp in microseconds.
    pub timestamp_us: u64,
}

impl TriggerEvent {
    /// Build a trigger event.
    #[must_use]
    pub const fn new(
        pad: PadId,
        velocity: Velocity,
        source: TriggerSource,
        timestamp_us: u64,
    ) -> Self {
        Self {
            pad,
            velocity,
            source,
            timestamp_us,
        }
    }
}

/// A held pad was released (gate-mode playback).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseEvent {
    /// Pad that was released.
    pub pad: PadId,
    /// Capture path that produced the event.
    pub source: TriggerSource,
    /// Monotonic capture timestamp in microseconds.
    pub timestamp_us: u64,
}

impl ReleaseEvent {
    /// Build a release event.
    #[must_use]
    pub const fn new(pad: PadId, source: TriggerSource, timestamp_us: u64) -> Self {
        Self {
            pad,
            source,
            timestamp_us,
        }
    }
}

/// One event on the shared input path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputEvent {
    /// A pad hit.
    Trigger(TriggerEvent),
    /// A gate-mode release.
    Release(ReleaseEvent),
}

impl InputEvent {
    /// Pad the event applies to.
    #[must_use]
    pub const fn pad(self) -> PadId {
        match self {
            Self::Trigger(event) => event.pad,
            Self::Release(event) => event.pad,
        }
    }

    /// Capture path that produced the event.
    #[must_use]
    pub const fn source(self) -> TriggerSource {
        match self {
            Self::Trigger(event) => event.source,
            Self::Release(event) => event.source,
        }
    }

    /// Monotonic capture timestamp in microseconds.
    #[must_use]
    pub const fn timestamp_us(self) -> u64 {
        match self {
            Self::Trigger(event) => event.timestamp_us,
            Self::Release(event) => event.timestamp_us,
        }
    }

    /// True for a [`InputEvent::Trigger`].
    #[must_use]
    pub const fn is_trigger(self) -> bool {
        matches!(self, Self::Trigger(_))
    }

    /// True for a [`InputEvent::Release`].
    #[must_use]
    pub const fn is_release(self) -> bool {
        matches!(self, Self::Release(_))
    }
}

impl From<TriggerEvent> for InputEvent {
    fn from(event: TriggerEvent) -> Self {
        Self::Trigger(event)
    }
}

impl From<ReleaseEvent> for InputEvent {
    fn from(event: ReleaseEvent) -> Self {
        Self::Release(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_ids_are_zero_based_and_bounded() {
        assert_eq!(PadId::new(0).unwrap().index(), 0);
        assert_eq!(PadId::new(15).unwrap().index(), 15);
        assert!(PadId::new(16).is_err());
        assert_eq!(PadId::ALL.len(), PAD_COUNT);
        for (index, pad) in PadId::ALL.iter().enumerate() {
            assert_eq!(pad.index(), index);
            assert_eq!(pad.get(), index as u8);
        }
    }

    #[test]
    fn pad_label_is_one_based_for_people() {
        assert_eq!(pad_label(PadId::from_index(0)), "pad 1");
        assert_eq!(pad_label(PadId::from_index(15)), "pad 16");
    }

    #[test]
    fn velocity_edges_are_validated() {
        assert!(Velocity::new(0).is_err());
        assert_eq!(Velocity::new(1), Ok(MIN_VELOCITY));
        assert_eq!(Velocity::new(127), Ok(MAX_VELOCITY));
        assert!(Velocity::new(128).is_err());
        assert_eq!(Velocity::clamp_signed(-3), MIN_VELOCITY);
        assert_eq!(Velocity::clamp_signed(0), MIN_VELOCITY);
        assert_eq!(Velocity::clamp_signed(64), Velocity::new(64).unwrap());
        assert_eq!(Velocity::clamp_signed(999), MAX_VELOCITY);
    }

    #[test]
    fn midi_velocity_zero_is_note_off() {
        assert!(Velocity::from_midi(0).is_none());
        assert_eq!(Velocity::from_midi(1), Some(MIN_VELOCITY));
    }

    #[test]
    fn normalized_velocity_spans_zero_to_one() {
        assert_eq!(MIN_VELOCITY.normalized(), 0.0);
        assert_eq!(
            Velocity::new(64).unwrap().normalized(),
            0.5,
            "1..=127 normalizes to 0..=1 with 64 at the midpoint"
        );
        assert_eq!(Velocity::MAXIMUM.normalized(), 1.0);
    }

    #[test]
    fn event_types_use_the_shared_model_identifiers() {
        let model_pad = drumpad_model::PadId::new(3).unwrap();
        let model_velocity = drumpad_model::Velocity::new(90).unwrap();
        let event = TriggerEvent::new(model_pad, model_velocity, TriggerSource::Keyboard, 7);
        assert_eq!(event.pad, model_pad);
        assert_eq!(event.velocity, model_velocity);
        let release = ReleaseEvent::new(model_pad, TriggerSource::Midi, 8);
        assert_eq!(release.pad, model_pad);
    }

    #[test]
    fn trigger_and_release_share_one_event_type() {
        let pad = PadId::from_index(3);
        let trigger: InputEvent = TriggerEvent::new(
            pad,
            Velocity::new(90).unwrap(),
            TriggerSource::Keyboard,
            1_000,
        )
        .into();
        let release: InputEvent = ReleaseEvent::new(pad, TriggerSource::Midi, 2_000).into();

        assert!(trigger.is_trigger());
        assert!(release.is_release());
        assert_eq!(trigger.pad(), release.pad());
        assert_eq!(trigger.source(), TriggerSource::Keyboard);
        assert_eq!(release.source(), TriggerSource::Midi);
        assert_eq!(trigger.timestamp_us(), 1_000);
        assert_eq!(release.timestamp_us(), 2_000);
    }

    #[test]
    fn events_round_trip_through_json() {
        let event = InputEvent::Trigger(TriggerEvent::new(
            PadId::from_index(15),
            Velocity::new(127).unwrap(),
            TriggerSource::Midi,
            42,
        ));
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"trigger","pad":15,"velocity":127,"source":"midi","timestamp_us":42}"#
        );
        assert_eq!(serde_json::from_str::<InputEvent>(&json).unwrap(), event);
    }
}
