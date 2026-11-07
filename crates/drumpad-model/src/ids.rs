//! Canonical pad and velocity identifiers shared by every Drumpad crate.
//!
//! `PadId` and `Velocity` live here, not in `kit`, so the input and engine
//! crates can depend on the same permanent types without importing the whole
//! kit document model. The tuple fields are public as agreed in the
//! cross-crate interface contract; [`PadId::new`] and [`Velocity::new`] are the
//! validating constructors, and the `clamped` helpers are available when a
//! lossy correction is explicitly acceptable.
//!
//! Both types serialize as plain integers, matching their on-disk and MIDI
//! representation.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Number of playable pads in a v1 kit.
pub const PAD_COUNT: usize = 16;
/// Smallest valid MIDI velocity.
pub const MIN_VELOCITY: u8 = 1;
/// Largest valid MIDI velocity.
pub const MAX_VELOCITY: u8 = 127;

/// Errors from constructing a shared model identifier.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Error)]
pub enum ModelError {
    /// A pad id was outside the 0..=15 range.
    #[error("pad id {value} is outside 0..={max}")]
    InvalidPadId {
        /// Rejected value.
        value: u8,
        /// Largest accepted pad id.
        max: u8,
    },
    /// A velocity was outside the 1..=127 range.
    #[error("velocity {value} is outside {min}..={max}")]
    InvalidVelocity {
        /// Rejected value.
        value: u8,
        /// Smallest accepted velocity.
        min: u8,
        /// Largest accepted velocity.
        max: u8,
    },
}

/// Stable, zero-based identifier for one of the 16 pads.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PadId(pub u8);

impl PadId {
    /// Largest valid pad id.
    pub const MAX: u8 = (PAD_COUNT - 1) as u8;
    /// Number of valid pad ids.
    pub const COUNT: usize = PAD_COUNT;
    /// First pad in performance order.
    pub const FIRST: Self = Self(0);

    /// Construct after checking the 0..=15 bound.
    pub fn new(value: u8) -> Result<Self, ModelError> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(ModelError::InvalidPadId {
                value,
                max: Self::MAX,
            })
        }
    }

    /// Construct from a value the caller already validated.
    pub const fn from_raw_unchecked(value: u8) -> Self {
        debug_assert!(value <= Self::MAX);
        Self(value)
    }

    /// Clamp an out-of-range value into the valid range.
    pub const fn clamped(value: u8) -> Self {
        Self(if value > Self::MAX { Self::MAX } else { value })
    }

    /// Raw zero-based value.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Zero-based index suitable for array access.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl Default for PadId {
    fn default() -> Self {
        Self::FIRST
    }
}

impl fmt::Display for PadId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Serialize for PadId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.0)
    }
}

impl<'de> Deserialize<'de> for PadId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let raw = u8::deserialize(deserializer)?;
        Self::new(raw).map_err(D::Error::custom)
    }
}

/// MIDI velocity in the inclusive 1..=127 range.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Velocity(pub u8);

impl Velocity {
    /// Smallest valid velocity.
    pub const MIN: u8 = MIN_VELOCITY;
    /// Largest valid velocity.
    pub const MAX: u8 = MAX_VELOCITY;
    /// Conventional default for velocity-sensitive triggers.
    pub const DEFAULT: Self = Self(100);
    /// Loudest possible hit.
    pub const MAXIMUM: Self = Self(Self::MAX);

    /// Construct after checking the 1..=127 bound.
    pub fn new(value: u8) -> Result<Self, ModelError> {
        if (Self::MIN..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ModelError::InvalidVelocity {
                value,
                min: Self::MIN,
                max: Self::MAX,
            })
        }
    }

    /// Construct from a value the caller already validated.
    pub const fn from_raw_unchecked(value: u8) -> Self {
        debug_assert!(value >= Self::MIN && value <= Self::MAX);
        Self(value)
    }

    /// Clamp an out-of-range value into the valid range.
    pub const fn clamped(value: u8) -> Self {
        if value < Self::MIN {
            Self(Self::MIN)
        } else if value > Self::MAX {
            Self(Self::MAX)
        } else {
            Self(value)
        }
    }

    /// Raw MIDI velocity.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Fraction in `0.0..=1.0`, useful for gain curves.
    pub fn normalized(self) -> f32 {
        f32::from(self.0 - Self::MIN) / f32::from(Self::MAX - Self::MIN)
    }
}

impl Default for Velocity {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Display for Velocity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Serialize for Velocity {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.0)
    }
}

impl<'de> Deserialize<'de> for Velocity {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let raw = u8::deserialize(deserializer)?;
        Self::new(raw).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_ids_accept_only_the_sixteen_slots() {
        assert_eq!(PadId::new(0).unwrap(), PadId::FIRST);
        assert_eq!(PadId::new(15).unwrap(), PadId(15));
        assert_eq!(
            PadId::new(16),
            Err(ModelError::InvalidPadId { value: 16, max: 15 })
        );
        assert_eq!(PadId::clamped(99), PadId(15));
        assert_eq!(PadId::default(), PadId::FIRST);
    }

    #[test]
    fn velocities_accept_only_the_midi_range() {
        assert_eq!(Velocity::new(1).unwrap(), Velocity(1));
        assert_eq!(Velocity::new(127).unwrap(), Velocity::MAXIMUM);
        assert_eq!(
            Velocity::new(0),
            Err(ModelError::InvalidVelocity {
                value: 0,
                min: 1,
                max: 127
            })
        );
        assert_eq!(
            Velocity::new(128),
            Err(ModelError::InvalidVelocity {
                value: 128,
                min: 1,
                max: 127
            })
        );
        assert_eq!(Velocity::clamped(0), Velocity(1));
        assert_eq!(Velocity::clamped(255), Velocity(127));
        assert_eq!(Velocity::default(), Velocity::DEFAULT);
    }

    #[test]
    fn serde_round_trips_as_integers_and_rejects_invalid_values() {
        let pad = PadId(7);
        assert_eq!(serde_json::to_string(&pad).unwrap(), "7");
        assert_eq!(serde_json::from_str::<PadId>("7").unwrap(), pad);
        assert!(serde_json::from_str::<PadId>("16").is_err());

        let velocity = Velocity(64);
        assert_eq!(serde_json::to_string(&velocity).unwrap(), "64");
        assert_eq!(serde_json::from_str::<Velocity>("64").unwrap(), velocity);
        assert!(serde_json::from_str::<Velocity>("0").is_err());
        assert!(serde_json::from_str::<Velocity>("128").is_err());
    }

    #[test]
    fn velocity_normalization_covers_the_full_range() {
        assert_eq!(Velocity(1).normalized(), 0.0);
        assert_eq!(Velocity(127).normalized(), 1.0);
        assert!(Velocity(64).normalized() > 0.4);
    }
}
